// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE set-selection path the `listBy` listings share.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every row below is one case of: a listing taking `listBy` and
//! `category` reports the built-in records when `listBy` names the built-in set and the
//! account's own records for every other value of it (including none at all, which names
//! only an ordering key), keeping only the records stored under exactly the `category` the
//! request names, and keeping every one of them when it names none.
//!
//! The rows span the two operations that reach that path — `ListJobTemplates` and
//! `ListPresets` — against both kinds of record, the three `listBy` values, both `Order`
//! directions, a category two records share, a category one record holds, a record stored
//! under no category at all, and categories that differ from a stored one only in case or
//! by a suffix. So a change to which set a `listBy` value selects, or to how `category`
//! matches, cannot land in one listing and miss the other.
//!
//! The account's own records are seeded straight into the store, as the listing suites
//! beside this one do; the write side's own coverage lives in
//! `tests/all/job_template_crud.rs` and `tests/all/preset_lifecycle.rs`. The built-in
//! records are the ones the account comes up with, and their names are read out of the
//! store rather than written down here. Nothing else here reaches past HTTP.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::{
    ContainerSettings, JobTemplateSettings, PresetSettings,
};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    default_acceleration_settings, JobTemplateRecord, PresetRecord, DEFAULT_PRIORITY,
    DEFAULT_STATUS_UPDATE_INTERVAL, TYPE_CUSTOM, TYPE_SYSTEM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::{
    ComAmazonawsMediaconvertMediaconvertState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header in this service's scope. It is what the router reads to
/// pick the service, and what a handler reads the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// One HTTP reply, decoded far enough to assert on.
struct Reply {
    status: u16,
    /// The JSON body, or `Value::Null` for an empty one.
    body: Value,
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn get(&self, uri: &str) -> Reply {
        let request = ::http::Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", format!("mediaconvert.{REGION}.amazonaws.com"))
            .header("authorization", authorization(REGION))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }
}

/// An instant `seconds_ago` before now, for seeding a creation order that is deliberately
/// not the alphabetical one.
fn ago(seconds_ago: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds_ago)
}

/// Put one of the account's own ([`TYPE_CUSTOM`]) job templates in the store.
fn seed_job_template(
    emulator: &Emulator,
    name: &str,
    category: Option<&str>,
    created_at: SystemTime,
) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .insert(
            name.to_string(),
            JobTemplateRecord {
                name: name.to_string(),
                category: category.map(str::to_string),
                description: Some(format!("{name} description")),
                queue: None,
                priority: DEFAULT_PRIORITY,
                settings: JobTemplateSettings::default(),
                status_update_interval: DEFAULT_STATUS_UPDATE_INTERVAL.to_string(),
                acceleration_settings: default_acceleration_settings(),
                elemental_inference_settings: None,
                hop_destinations: Vec::new(),
                template_type: TYPE_CUSTOM.to_string(),
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// Put one of the account's own ([`TYPE_CUSTOM`]) presets in the store.
fn seed_preset(emulator: &Emulator, name: &str, category: Option<&str>, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .presets
        .insert(
            name.to_string(),
            PresetRecord {
                name: name.to_string(),
                category: category.map(str::to_string),
                description: Some(format!("{name} description")),
                // A stored preset names the container of the output it describes
                // (`ContainerSettings$Container`), as one created over HTTP has to.
                settings: PresetSettings {
                    container_settings: Some(ContainerSettings {
                        container: Some("MP4".to_string()),
                        ..ContainerSettings::default()
                    }),
                    ..PresetSettings::default()
                },
                preset_type: TYPE_CUSTOM.to_string(),
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// The names of the stored job templates of one `Type`.
fn job_template_names_of_type(emulator: &Emulator, record_type: &str) -> Vec<String> {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .values()
        .filter(|record| record.template_type == record_type)
        .map(|record| record.name.clone())
        .collect()
}

/// The names of the stored presets of one `Type`.
fn preset_names_of_type(emulator: &Emulator, record_type: &str) -> Vec<String> {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .presets
        .values()
        .filter(|record| record.preset_type == record_type)
        .map(|record| record.name.clone())
        .collect()
}

/// One of the two listings that select their set from `listBy` and `category`.
///
/// The bindings come from the model (`smithy_query(query="digest")`):
/// `GET /2017-08-29/jobTemplates` and `GET /2017-08-29/presets`.
struct Listing {
    /// How a failing assertion names this listing.
    label: &'static str,
    /// The operation's HTTP URI.
    path: &'static str,
    /// The output member the records come back in.
    items: &'static str,
    /// Put one of the account's own records of this kind in the store.
    seed: fn(&Emulator, &str, Option<&str>, SystemTime),
    /// The names of the stored records of this kind carrying one `Type`.
    names_of_type: fn(&Emulator, &str) -> Vec<String>,
}

/// Both listings that take `listBy` and `category`.
const LISTINGS: [Listing; 2] = [
    Listing {
        label: "ListJobTemplates",
        path: "/2017-08-29/jobTemplates",
        items: "jobTemplates",
        seed: seed_job_template,
        names_of_type: job_template_names_of_type,
    },
    Listing {
        label: "ListPresets",
        path: "/2017-08-29/presets",
        items: "presets",
        seed: seed_preset,
        names_of_type: preset_names_of_type,
    },
];

impl Listing {
    /// One page of this listing for `query`.
    async fn list(&self, emulator: &Emulator, query: &str) -> Reply {
        let uri = if query.is_empty() {
            self.path.to_string()
        } else {
            format!("{}?{}", self.path, query)
        };
        emulator.get(&uri).await
    }

    /// The records one reply reported, in the order they came back.
    fn items(&self, reply: &Reply) -> Vec<Value> {
        reply
            .body
            .pointer(&format!("/{}", self.items))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The names one reply reported, in the order they came back.
    fn names(&self, reply: &Reply) -> Vec<String> {
        self.items(reply)
            .iter()
            .map(|item| {
                item.pointer("/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// Seed the account's own records every case below selects from: four of them, whose
    /// alphabetical order and creation order deliberately differ, two sharing a category,
    /// one in a category of its own and one stored under no category at all.
    fn seed_all(&self, emulator: &Emulator) {
        (self.seed)(emulator, "zeta-selection", Some("promos"), ago(400));
        (self.seed)(emulator, "alpha-selection", Some("promos"), ago(300));
        (self.seed)(emulator, "mid-selection", Some("ott-hls"), ago(200));
        (self.seed)(emulator, "uncat-selection", None, ago(100));
    }

    /// The built-in records' names, alphabetically — read from the store, so no case
    /// hard-codes the catalogue the account comes up with.
    fn built_in_names(&self, emulator: &Emulator) -> Vec<String> {
        let mut names = (self.names_of_type)(emulator, TYPE_SYSTEM);
        names.sort();
        names
    }
}

/// The names a row is due.
enum Selection {
    /// Exactly these, in this order.
    Named(&'static [&'static str]),
    /// The built-in catalogue, alphabetically, or its reverse. Every built-in record is
    /// seeded at the same instant, so the one total order a case can predict is the
    /// alphabetical one.
    BuiltIns { descending: bool },
}

/// One row of the selection table.
struct Case {
    /// The query string the request carries.
    query: &'static str,
    /// The `Type` every record the row reports must carry.
    expected_type: &'static str,
    /// The `category` the request named, which every reported record must then carry.
    category: Option<&'static str>,
    /// The names the row is due.
    expected: Selection,
}

/// The account's own records, by name and by creation instant, as [`Listing::seed_all`]
/// stores them.
const BY_NAME: &[&str] = &[
    "alpha-selection",
    "mid-selection",
    "uncat-selection",
    "zeta-selection",
];
const BY_CREATION: &[&str] = &[
    "zeta-selection",
    "alpha-selection",
    "mid-selection",
    "uncat-selection",
];
const BY_CREATION_DESCENDING: &[&str] = &[
    "uncat-selection",
    "mid-selection",
    "alpha-selection",
    "zeta-selection",
];
/// The two records sharing a category, in name order.
const PROMOS_BY_NAME: &[&str] = &["alpha-selection", "zeta-selection"];
/// The same two in creation order.
const PROMOS_BY_CREATION: &[&str] = &["zeta-selection", "alpha-selection"];
/// Nothing at all.
const NONE: &[&str] = &[];

/// Every row of the set-selection table.
fn cases() -> Vec<Case> {
    vec![
        // No `listBy`: the account's own records, on the key each listing defaults to.
        Case {
            query: "",
            expected_type: TYPE_CUSTOM,
            category: None,
            expected: Selection::Named(BY_NAME),
        },
        Case {
            query: "listBy=NAME",
            expected_type: TYPE_CUSTOM,
            category: None,
            expected: Selection::Named(BY_NAME),
        },
        Case {
            query: "listBy=CREATION_DATE",
            expected_type: TYPE_CUSTOM,
            category: None,
            expected: Selection::Named(BY_CREATION),
        },
        Case {
            query: "listBy=CREATION_DATE&order=DESCENDING",
            expected_type: TYPE_CUSTOM,
            category: None,
            expected: Selection::Named(BY_CREATION_DESCENDING),
        },
        // `SYSTEM` selects the built-in set, and none of the account's own records.
        Case {
            query: "listBy=SYSTEM",
            expected_type: TYPE_SYSTEM,
            category: None,
            expected: Selection::BuiltIns { descending: false },
        },
        Case {
            query: "listBy=SYSTEM&order=DESCENDING",
            expected_type: TYPE_SYSTEM,
            category: None,
            expected: Selection::BuiltIns { descending: true },
        },
        // `category` keeps only the records stored under exactly it, on whichever set and
        // key the rest of the request named.
        Case {
            query: "category=promos",
            expected_type: TYPE_CUSTOM,
            category: Some("promos"),
            expected: Selection::Named(PROMOS_BY_NAME),
        },
        Case {
            query: "listBy=NAME&category=promos",
            expected_type: TYPE_CUSTOM,
            category: Some("promos"),
            expected: Selection::Named(PROMOS_BY_NAME),
        },
        Case {
            query: "listBy=CREATION_DATE&category=promos",
            expected_type: TYPE_CUSTOM,
            category: Some("promos"),
            expected: Selection::Named(PROMOS_BY_CREATION),
        },
        Case {
            query: "category=ott-hls",
            expected_type: TYPE_CUSTOM,
            category: Some("ott-hls"),
            expected: Selection::Named(&["mid-selection"]),
        },
        // The match is exact: neither a different case, nor a prefix, nor a suffix of a
        // stored category selects the record holding it.
        Case {
            query: "category=Promos",
            expected_type: TYPE_CUSTOM,
            category: Some("Promos"),
            expected: Selection::Named(NONE),
        },
        Case {
            query: "category=promo",
            expected_type: TYPE_CUSTOM,
            category: Some("promo"),
            expected: Selection::Named(NONE),
        },
        Case {
            query: "category=promos-extra",
            expected_type: TYPE_CUSTOM,
            category: Some("promos-extra"),
            expected: Selection::Named(NONE),
        },
        // A record stored under no category belongs to no category a caller can name, so
        // the built-in set — none of whose records carries one — is empty under any
        // `category`.
        Case {
            query: "listBy=SYSTEM&category=promos",
            expected_type: TYPE_SYSTEM,
            category: Some("promos"),
            expected: Selection::BuiltIns { descending: false },
        },
    ]
}

/// The names a row is due of this listing.
fn expected_names(listing: &Listing, emulator: &Emulator, case: &Case) -> Vec<String> {
    match &case.expected {
        Selection::Named(names) => names.iter().map(|name| (*name).to_string()).collect(),
        Selection::BuiltIns { descending } => {
            // A `category` narrows the built-in set to nothing: no built-in record is
            // stored under a category.
            if case.category.is_some() {
                return Vec::new();
            }
            let mut names = listing.built_in_names(emulator);
            if *descending {
                names.reverse();
            }
            names
        }
    }
}

/// `listBy` and `category` select the same set in both listings: every row of the table
/// above, against job templates and against presets.
#[tokio::test]
async fn list_by_and_category_select_the_same_set_in_every_listing() {
    for listing in &LISTINGS {
        let emulator = Emulator::new();
        listing.seed_all(&emulator);

        for case in cases() {
            let label = format!("{} {:?}", listing.label, case.query);
            let reply = listing.list(&emulator, case.query).await;
            assert_eq!(
                reply.status, 200,
                "case {label}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let expected = expected_names(listing, &emulator, &case);
            let names = listing.names(&reply);
            assert_eq!(
                names, expected,
                "case {label}: expected {expected:?}, got {names:?}"
            );
            for item in listing.items(&reply) {
                assert_eq!(
                    item.pointer("/type").and_then(Value::as_str),
                    Some(case.expected_type),
                    "case {label}: expected type {:?}, got {item}",
                    case.expected_type
                );
                if let Some(category) = case.category {
                    assert_eq!(
                        item.pointer("/category").and_then(Value::as_str),
                        Some(category),
                        "case {label}: expected category {category:?}, got {item}"
                    );
                }
            }
        }
    }
}

/// The two listings select identically: given the same records under the same names,
/// categories and creation instants, every row reports the same names from job templates as
/// it does from presets — so neither listing can drift from the other.
#[tokio::test]
async fn both_listings_select_the_account_s_own_records_identically() {
    let emulator = Emulator::new();
    for listing in &LISTINGS {
        listing.seed_all(&emulator);
    }

    for case in cases() {
        // The built-in catalogues are two different sets of records, so only the rows
        // selecting the account's own records — the ones seeded identically into both
        // stores — are comparable name for name.
        if matches!(case.expected, Selection::BuiltIns { .. }) {
            continue;
        }
        let mut reported: Vec<(&str, Vec<String>)> = Vec::new();
        for listing in &LISTINGS {
            let reply = listing.list(&emulator, case.query).await;
            assert_eq!(
                reply.status, 200,
                "case {} {:?}: expected HTTP 200, got {} with body {}",
                listing.label, case.query, reply.status, reply.body
            );
            reported.push((listing.label, listing.names(&reply)));
        }
        let Some(((first_label, first), rest)) = reported.split_first() else {
            continue;
        };
        for (label, names) in rest {
            assert_eq!(
                names, first,
                "case {:?}: {label} reported {names:?}, but {first_label} reported {first:?}",
                case.query
            );
        }
    }
}
