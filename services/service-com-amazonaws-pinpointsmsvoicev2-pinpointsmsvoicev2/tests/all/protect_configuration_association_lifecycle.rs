// SPDX-License-Identifier: CC0-1.0

//! The lifetime of the protect-configuration link on a configuration set, end to end over
//! the wire.
//!
//! Every request goes through [`MultiServiceRouter::dispatch`] as `awsJson1_0` — `POST /`
//! with `X-Amz-Target: PinpointSMSVoiceV2.<Op>` — so these tests exercise routing,
//! deserialization, constraint validation, the handler and serialization, exactly as a
//! real SDK client would reach them.
//!
//! The rule under test: the link `AssociateProtectConfiguration` records is one member of
//! the configuration-set record, so it is independent of every other member of that set and
//! lives exactly as long as the set — changing the set's event destinations leaves the link
//! as it was, changing the link leaves those destinations as they were, and deleting the set
//! takes the link with it, so no later read reports it and a set created again under the same
//! name carries none.
//!
//! The sibling suite `origination_identity_and_send_text` pins what one associate/disassociate
//! pair reports and persists; this suite pins the link's INDEPENDENCE from the rest of the set
//! and its lifetime. Its harness is its own, as every sibling suite's is: their helpers are
//! private to their modules.

use std::sync::Arc;

use service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2 as sms_voice;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for, so the ARNs under assertion are not the default one.
const REGION: &str = "ap-southeast-2";

/// The account id every emulated ARN carries.
const ACCOUNT_ID: &str = emulator_server::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus a router with only this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(sms_voice::EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and answer `(http status, parsed JSON body)`.
    async fn call(&self, operation: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sms-voice/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require HTTP 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: serde_json::Value) -> serde_json::Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create one configuration set under the given name.
    async fn create_configuration_set(&self, name: &str) {
        self.ok(
            "CreateConfigurationSet",
            serde_json::json!({"ConfigurationSetName": name}),
        )
        .await;
    }

    /// Create one protect configuration and answer its minted id.
    async fn create_protect_configuration(&self) -> String {
        let created = self
            .ok(
                "CreateProtectConfiguration",
                serde_json::json!({"DeletionProtectionEnabled": false}),
            )
            .await;
        member(&created, "ProtectConfigurationId")
    }

    /// Link one stored protect configuration to one stored configuration set, requiring the
    /// response to report the stored identity of both ends of the link it recorded.
    async fn associate(&self, set: &str, protect_configuration_id: &str) {
        let associated = self
            .ok(
                "AssociateProtectConfiguration",
                serde_json::json!({
                    "ConfigurationSetName": set,
                    "ProtectConfigurationId": protect_configuration_id,
                }),
            )
            .await;
        assert_eq!(
            (
                member(&associated, "ConfigurationSetName"),
                member(&associated, "ConfigurationSetArn"),
                member(&associated, "ProtectConfigurationId"),
                member(&associated, "ProtectConfigurationArn"),
            ),
            (
                set.to_string(),
                configuration_set_arn(set),
                protect_configuration_id.to_string(),
                protect_configuration_arn(protect_configuration_id),
            ),
            "case {set:?}/{protect_configuration_id:?}: expected the region- and \
             account-scoped identity of both resources, got {associated}"
        );
    }

    /// Every stored configuration set, as the whole-account describe reports them.
    async fn described_sets(&self) -> Vec<serde_json::Value> {
        let described = self
            .ok("DescribeConfigurationSets", serde_json::json!({}))
            .await;
        described
            .get("ConfigurationSets")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("expected a ConfigurationSets list in {described}"))
    }

    /// The record one stored set reports, or `None` when no stored set carries that name.
    async fn described_set(&self, set: &str) -> Option<serde_json::Value> {
        self.described_sets()
            .await
            .into_iter()
            .find(|entry| member(entry, "ConfigurationSetName") == set)
    }

    /// The protect configuration one stored set reports, or `None` when it reports none.
    async fn link(&self, set: &str) -> Option<String> {
        let entry = self
            .described_set(set)
            .await
            .unwrap_or_else(|| panic!("expected a stored configuration set named {set:?}"));
        entry
            .get("ProtectConfigurationId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    /// The event destinations one stored set reports, in the order reported.
    async fn destinations(&self, set: &str) -> Vec<serde_json::Value> {
        let entry = self
            .described_set(set)
            .await
            .unwrap_or_else(|| panic!("expected a stored configuration set named {set:?}"));
        entry
            .get("EventDestinations")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The protect configurations the account holds, by id, in the order reported.
    async fn protect_configuration_ids(&self) -> Vec<String> {
        let described = self
            .ok("DescribeProtectConfigurations", serde_json::json!({}))
            .await;
        described
            .get("ProtectConfigurations")
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| member(entry, "ProtectConfigurationId"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A string member of a response body.
fn member(body: &serde_json::Value, name: &str) -> String {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {name} in {body}"))
        .to_string()
}

/// The wire error code of a failed call (`__type`, with any namespace prefix dropped).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The ARN one stored configuration set carries, as `CreateConfigurationSet` builds it from
/// the signing region and the emulator's account id.
fn configuration_set_arn(name: &str) -> String {
    format!("arn:aws:sms-voice:{REGION}:{ACCOUNT_ID}:configuration-set/{name}")
}

/// The ARN one stored protect configuration carries, built the same way.
fn protect_configuration_arn(id: &str) -> String {
    format!("arn:aws:sms-voice:{REGION}:{ACCOUNT_ID}:protect-configuration/{id}")
}

/// A `CreateEventDestination` request carrying the smallest destination the model declares.
fn create_destination(set: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "ConfigurationSetName": set,
        "EventDestinationName": name,
        "MatchingEventTypes": ["ALL"],
        "SnsDestination": {"TopicArn": "arn:aws:sns:ap-southeast-2:111122223333:sms-voice-events"},
    })
}

/// A `DeleteEventDestination` request for one destination of one set.
fn delete_destination(set: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"ConfigurationSetName": set, "EventDestinationName": name})
}

/// The names of a reported list of event destinations, in the order reported.
fn destination_names(destinations: &[serde_json::Value]) -> Vec<String> {
    destinations
        .iter()
        .map(|destination| member(destination, "EventDestinationName"))
        .collect()
}

#[tokio::test]
async fn the_protect_configuration_link_is_independent_of_the_sets_event_destinations() {
    let emulator = Emulator::new();
    let protect = emulator.create_protect_configuration().await;
    let set = "guarded-set";
    emulator.create_configuration_set(set).await;
    emulator.associate(set, &protect).await;

    // One row per shape the set's event-destination list can be driven into: filled, grown,
    // shrunk and emptied again. `UpdateEventDestination` is not a row: it is still a
    // scaffold in this emulator, so a row for it would assert on a 501.
    let rows: Vec<(&str, &str, serde_json::Value, Vec<&str>)> = vec![
        (
            "the first destination added",
            "CreateEventDestination",
            create_destination(set, "alpha"),
            vec!["alpha"],
        ),
        (
            "a second destination added",
            "CreateEventDestination",
            create_destination(set, "beta"),
            vec!["alpha", "beta"],
        ),
        (
            "a third destination added",
            "CreateEventDestination",
            create_destination(set, "gamma"),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "a destination removed from the middle",
            "DeleteEventDestination",
            delete_destination(set, "beta"),
            vec!["alpha", "gamma"],
        ),
        (
            "another destination removed",
            "DeleteEventDestination",
            delete_destination(set, "alpha"),
            vec!["gamma"],
        ),
        (
            "the list emptied",
            "DeleteEventDestination",
            delete_destination(set, "gamma"),
            vec![],
        ),
    ];

    for (case, operation, request, expected) in rows {
        emulator.ok(operation, request).await;
        let destinations = emulator.destinations(set).await;
        assert_eq!(
            destination_names(&destinations),
            expected,
            "case {case:?}: expected the destinations the change left, got {destinations:?}"
        );
        // The subject of this suite: nothing done to the set's other members disturbs its
        // protect-configuration link.
        assert_eq!(
            emulator.link(set).await,
            Some(protect.clone()),
            "case {case:?}: the set must still report the protect configuration it is \
             associated with"
        );
    }
}

#[tokio::test]
async fn changing_the_protect_configuration_link_leaves_the_sets_event_destinations_alone() {
    let emulator = Emulator::new();
    let first = emulator.create_protect_configuration().await;
    let second = emulator.create_protect_configuration().await;
    let set = "shared-set";
    emulator.create_configuration_set(set).await;
    for name in ["alpha", "beta"] {
        emulator
            .ok("CreateEventDestination", create_destination(set, name))
            .await;
    }
    let before = emulator.destinations(set).await;
    assert_eq!(
        destination_names(&before),
        vec!["alpha".to_string(), "beta".to_string()],
        "the set under test starts out with both of its event destinations, got {before:?}"
    );

    // One row per shape the link can be driven into — set, replaced, cleared, set again —
    // each with the id the set must report afterwards.
    let rows: Vec<(&str, &str, Option<&String>)> = vec![
        (
            "the first association",
            "AssociateProtectConfiguration",
            Some(&first),
        ),
        (
            "a second protect configuration replacing the first",
            "AssociateProtectConfiguration",
            Some(&second),
        ),
        (
            "the link cleared",
            "DisassociateProtectConfiguration",
            None,
        ),
        (
            "the first protect configuration associated again",
            "AssociateProtectConfiguration",
            Some(&first),
        ),
    ];

    for (case, operation, expected) in rows {
        // A disassociate names the link it clears, so the row that clears it names the id
        // the previous row left on the set.
        let named = expected.unwrap_or(&second);
        emulator
            .ok(
                operation,
                serde_json::json!({
                    "ConfigurationSetName": set,
                    "ProtectConfigurationId": named,
                }),
            )
            .await;
        assert_eq!(
            emulator.link(set).await.as_ref(),
            expected,
            "case {case:?}: the set must report exactly the link the call left"
        );
        let after = emulator.destinations(set).await;
        assert_eq!(
            after, before,
            "case {case:?}: a change to the link must leave every event destination of the \
             set exactly as it was, got {after:?}"
        );
    }
}

/// One row of the deletion table: the state the subject set is deleted in.
struct DeletionCase {
    case: &'static str,
    /// How many event destinations the subject set carries when it is deleted.
    destinations: usize,
    /// Whether the subject's link was replaced by a second protect configuration, so the
    /// link deleted with the set is not the first one it ever held.
    replaced: bool,
    /// How many OTHER sets hold a link to the first protect configuration, so the delete's
    /// effect can be told apart from an account-wide one.
    sibling_sets: usize,
}

#[tokio::test]
async fn deleting_a_configuration_set_takes_its_protect_configuration_link_with_it() {
    let cases = [
        DeletionCase {
            case: "a bare set",
            destinations: 0,
            replaced: false,
            sibling_sets: 0,
        },
        DeletionCase {
            case: "a set that also carries event destinations",
            destinations: 2,
            replaced: false,
            sibling_sets: 0,
        },
        DeletionCase {
            case: "a set whose link was replaced before the delete",
            destinations: 1,
            replaced: true,
            sibling_sets: 0,
        },
        DeletionCase {
            case: "one of three sets linked to the same protect configuration",
            destinations: 0,
            replaced: false,
            sibling_sets: 2,
        },
    ];

    for case in &cases {
        let named = case.case;
        let emulator = Emulator::new();
        let first = emulator.create_protect_configuration().await;
        let second = emulator.create_protect_configuration().await;
        let subject = "subject-set";
        emulator.create_configuration_set(subject).await;
        for index in 0..case.destinations {
            emulator
                .ok(
                    "CreateEventDestination",
                    create_destination(subject, &format!("destination-{index}")),
                )
                .await;
        }
        let mut siblings = Vec::with_capacity(case.sibling_sets);
        for index in 0..case.sibling_sets {
            let name = format!("sibling-{index}");
            emulator.create_configuration_set(&name).await;
            emulator.associate(&name, &first).await;
            siblings.push(name);
        }
        emulator.associate(subject, &first).await;
        let doomed_link = if case.replaced {
            emulator.associate(subject, &second).await;
            second.clone()
        } else {
            first.clone()
        };
        assert_eq!(
            emulator.link(subject).await,
            Some(doomed_link.clone()),
            "case {named}: the set under test holds the link the delete must remove"
        );

        // DELETE the set. Its own response reports the record it removed, event
        // destinations and all.
        let deleted = emulator
            .ok(
                "DeleteConfigurationSet",
                serde_json::json!({"ConfigurationSetName": subject}),
            )
            .await;
        assert_eq!(
            member(&deleted, "ConfigurationSetName"),
            subject,
            "case {named}: the delete reports the set it removed, got {deleted}"
        );
        assert_eq!(
            deleted
                .get("EventDestinations")
                .and_then(serde_json::Value::as_array)
                .map(|destinations| destination_names(destinations).len())
                .unwrap_or(0),
            case.destinations,
            "case {named}: the delete reports the event destinations the set carried, got \
             {deleted}"
        );

        // The link is gone because the set is: no stored set reports it any more, and the
        // sibling sets' own links are untouched.
        // Sorted on both sides, so the assertion is about which links are left rather than
        // about the order the describe reports them in.
        let remaining: Vec<(String, Option<String>)> = {
            let mut rows = Vec::new();
            for entry in emulator.described_sets().await {
                let name = member(&entry, "ConfigurationSetName");
                let link = entry
                    .get("ProtectConfigurationId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                rows.push((name, link));
            }
            rows.sort();
            rows
        };
        let mut expected_remaining: Vec<(String, Option<String>)> = siblings
            .iter()
            .map(|name| (name.clone(), Some(first.clone())))
            .collect();
        expected_remaining.sort();
        assert_eq!(
            remaining, expected_remaining,
            "case {named}: only the sibling sets and their own links may be left"
        );

        // Both protect configurations outlive the set: deleting a set removes the LINK, not
        // the resource at its other end.
        let mut ids = emulator.protect_configuration_ids().await;
        ids.sort();
        let mut expected_ids = vec![first.clone(), second.clone()];
        expected_ids.sort();
        assert_eq!(
            ids, expected_ids,
            "case {named}: a set delete may not remove a protect configuration"
        );

        // A set created again under the same name is a NEW record: it carries neither the
        // link nor the event destinations of the one that was deleted.
        emulator.create_configuration_set(subject).await;
        assert_eq!(
            emulator.link(subject).await,
            None,
            "case {named}: a set created again under a deleted set's name reports no \
             protect configuration"
        );
        let recreated = emulator.destinations(subject).await;
        assert!(
            recreated.is_empty(),
            "case {named}: a set created again under a deleted set's name reports no event \
             destinations, got {recreated:?}"
        );

        // `DeleteProtectConfiguration` documents that a configuration associated with a
        // configuration set cannot be deleted, so whether each delete is served is exactly
        // whether a stored set still links it — which is what proves the deleted set's link
        // is gone from the record rather than merely unreported.
        let held: Vec<String> = remaining.iter().filter_map(|(_, link)| link.clone()).collect();
        for id in [&first, &second] {
            let (status, body) = emulator
                .call(
                    "DeleteProtectConfiguration",
                    serde_json::json!({"ProtectConfigurationId": id}),
                )
                .await;
            if held.contains(id) {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ConflictException"),
                    "case {named}: a configuration a stored set still links must refuse its \
                     delete, got {status} with {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {named}: a configuration no stored set links any more must be \
                     deletable, got {status} with {body}"
                );
            }
        }
    }
}
