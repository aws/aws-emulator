// SPDX-License-Identifier: CC0-1.0

//! The KEY a per-method throttle override of a usage plan's API stage association is held
//! under, end to end over the wire.
//!
//! The RULE under test: an entry of `ApiStage$throttle` — "a map containing method level
//! throttling information for API stage in a usage plan" — is keyed by the METHOD'S OWN
//! REFERENCE, the resource's path followed by the HTTP method, with the resource path
//! absolute (`/{resourcePath}/{httpMethod}`, and `//{httpMethod}` for a method of the ROOT
//! resource, whose path is `/` itself); the patch pointer
//! `/apiStages/{apiId}:{stage}/throttle/{resourcePath}/{httpMethod}/{rateLimit|burstLimit}`
//! addresses exactly that entry, whichever way the caller spells the resource path's leading
//! separator (bare, as the pointer's own separator, or escaped as `~1` inside one reference
//! token), and a `remove` of the pointer that ends at the method drops that entry again.
//! The plan's OWN `throttle` is a different member and no per-method patch touches it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, and every read-back is a later `GetUsagePlan` rather than the patch's own answer.
//! The emulator, the create helpers and the PATCH wrappers are this crate's existing
//! harness (`rest_api_lifecycle`, `stage_lifecycle`, `usage_plan_lifecycle`,
//! `patch_updates`).
//!
//! The rows differ along what the rule generalises over: how the pointer spells the
//! resource path (bare, `~1`-escaped, and the whole reference escaped inside one token),
//! how deep the resource path runs (the ROOT resource, whose own path is `/`, a top-level
//! resource, a nested one, one carrying a path parameter), the HTTP method, which limit the
//! pointer names, the op that writes it (`add` and `replace`, on an association that holds
//! the entry already and on one that does not), and the op that drops it (`remove`, on
//! either spelling).

use serde_json::{json, Value};

use crate::patch_updates::{op, patch_ok, replace};
use crate::rest_api_lifecycle::{create_api, string_member, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan};

/// The stage every row's association names, deployed through `CreateDeployment`.
const STAGE: &str = "test";

/// The plan-level throttle every row starts with, which no per-method patch may touch: a
/// different member of the plan from the association's own map.
const PLAN_RATE_LIMIT: f64 = 20.0;
const PLAN_BURST_LIMIT: i64 = 10;

/// One row: the case, the pointer tail that follows `…/throttle/` in each op, the ops the
/// row sends (built from that tail), and the whole `throttle` map the association must
/// report afterwards.
struct Row {
    case: &'static str,
    /// Whether the plan starts with the row's entry already in the map, so a row covers
    /// both an op that CREATES an entry and one that overwrites one.
    seeded: Option<fn() -> Value>,
    /// The ops of this row, as `(op kind, the pointer tail after `…/throttle/`, value)`.
    ops: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// The map `GetUsagePlan` must report for the association afterwards, as
    /// `(method reference, rateLimit, burstLimit)` triples.
    throttle: &'static [(&'static str, f64, i64)],
}

/// The seeded association map of a row that starts with one entry already written by the
/// CREATE — which is the other operation that writes this very map, so a row seeded through
/// it and patched here proves both reach the same key.
fn seeded_pets_get() -> Value {
    json!({ "/pets/GET": { "rateLimit": 1.0, "burstLimit": 1 } })
}

const ROWS: &[Row] = &[
    Row {
        // The spelling a client sends when it writes the resource path's leading separator
        // as the pointer's own separator: `…/throttle/pets/GET/rateLimit`.
        case: "the bare spelling of a top-level resource's method",
        seeded: None,
        ops: &[
            ("add", "pets/GET/rateLimit", Some("5")),
            ("add", "pets/GET/burstLimit", Some("2")),
        ],
        throttle: &[("/pets/GET", 5.0, 2)],
    },
    Row {
        // The same entry, addressed with the separator escaped inside one reference token.
        case: "the escaped spelling of the same method",
        seeded: None,
        ops: &[
            ("add", "~1pets/GET/rateLimit", Some("5")),
            ("add", "~1pets/GET/burstLimit", Some("2")),
        ],
        throttle: &[("/pets/GET", 5.0, 2)],
    },
    Row {
        // The whole reference inside one token, which is how a client that escapes every
        // separator spells it.
        case: "the whole method reference escaped inside one token",
        seeded: None,
        ops: &[("add", "~1pets~1GET/rateLimit", Some("3.5"))],
        throttle: &[("/pets/GET", 3.5, 0)],
    },
    Row {
        // Two spellings of ONE entry in one request: the second op must find the entry the
        // first wrote rather than start a second one.
        case: "two spellings of one entry in one request",
        seeded: None,
        ops: &[
            ("add", "pets/GET/rateLimit", Some("4")),
            ("add", "~1pets~1GET/burstLimit", Some("8")),
        ],
        throttle: &[("/pets/GET", 4.0, 8)],
    },
    Row {
        // A `replace` over the entry the CREATE wrote, in the bare spelling: the two
        // operations key the map the same way.
        case: "a replace of the entry the create wrote, bare",
        seeded: Some(seeded_pets_get),
        ops: &[("replace", "pets/GET/rateLimit", Some("9.5"))],
        throttle: &[("/pets/GET", 9.5, 1)],
    },
    Row {
        // A NESTED resource path: every segment between the map and the limit belongs to
        // the reference.
        case: "a nested resource path, bare",
        seeded: None,
        ops: &[("add", "pets/mine/POST/rateLimit", Some("6"))],
        throttle: &[("/pets/mine/POST", 6.0, 0)],
    },
    Row {
        // A path PARAMETER in the resource path, and a method that is not GET.
        case: "a path parameter in the resource path",
        seeded: None,
        ops: &[("add", "pets/{petId}/DELETE/burstLimit", Some("7"))],
        throttle: &[("/pets/{petId}/DELETE", 0.0, 7)],
    },
    Row {
        // Two different methods: each is its own entry of one association's map.
        case: "two methods of one association",
        seeded: None,
        ops: &[
            ("add", "pets/GET/rateLimit", Some("1")),
            ("add", "orders/{id}/PUT/rateLimit", Some("2")),
        ],
        throttle: &[("/orders/{id}/PUT", 2.0, 0), ("/pets/GET", 1.0, 0)],
    },
    Row {
        // The ROOT resource, whose path is `/` itself: its method's reference opens with two
        // separators, and the bare spelling leaves the first segment EMPTY.
        case: "the root resource's method, bare",
        seeded: None,
        ops: &[("add", "/GET/rateLimit", Some("2"))],
        throttle: &[("//GET", 2.0, 0)],
    },
    Row {
        // The same entry with the root path escaped into the first token, and a `remove` of
        // it in the bare spelling: the two agree on the one key.
        case: "the root resource's method, escaped, then removed bare",
        seeded: None,
        ops: &[
            ("add", "~1/GET/burstLimit", Some("3")),
            ("add", "~1~1GET/rateLimit", Some("4")),
            ("remove", "/GET", None),
        ],
        throttle: &[],
    },
    Row {
        // The `remove` the rule names, in the bare spelling: the pointer that ends at the
        // method drops that entry, and leaves the association's other entries alone.
        case: "a remove of one entry, bare",
        seeded: Some(seeded_pets_get),
        ops: &[
            ("add", "orders/PUT/rateLimit", Some("2")),
            ("remove", "pets/GET", None),
        ],
        throttle: &[("/orders/PUT", 2.0, 0)],
    },
    Row {
        // The same remove, escaped: both spellings address the entry the create wrote.
        case: "a remove of one entry, escaped",
        seeded: Some(seeded_pets_get),
        ops: &[("remove", "~1pets~1GET", None)],
        throttle: &[],
    },
    Row {
        // An entry written and then dropped in ONE request, each op in a different
        // spelling: what the read sees is the state the whole list left.
        case: "an entry added and removed in one request",
        seeded: None,
        ops: &[
            ("add", "~1pets/GET/rateLimit", Some("5")),
            ("remove", "pets/GET", None),
        ],
        throttle: &[],
    },
];

/// The `throttle` map a row expects, as the wire reports it.
fn expected_throttle(throttle: &[(&str, f64, i64)]) -> Value {
    Value::Object(
        throttle
            .iter()
            .map(|(method, rate_limit, burst_limit)| {
                (
                    (*method).to_string(),
                    json!({ "rateLimit": rate_limit, "burstLimit": burst_limit }),
                )
            })
            .collect(),
    )
}

#[tokio::test]
async fn a_per_method_throttle_is_keyed_by_the_methods_own_reference() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "usage-plan-method-throttle").await;
    // An association must name a stored stage, so the stage every row associates is
    // deployed first — over the wire, through `CreateDeployment`.
    create_deployed_stage(&gateway, &api, STAGE).await;

    for row in ROWS {
        let case = row.case;
        let mut stage = json!({ "apiId": api, "stage": STAGE });
        if let Some(seeded) = row.seeded {
            stage["throttle"] = seeded();
        }
        let created = create_usage_plan(
            &gateway,
            case,
            json!({
                "name": format!("plan-{}", case.replace(' ', "-")),
                "apiStages": [stage],
                "throttle": { "rateLimit": PLAN_RATE_LIMIT, "burstLimit": PLAN_BURST_LIMIT },
            }),
        )
        .await;
        let plan = string_member(&created, "id", case);

        let operations: Vec<Value> = row
            .ops
            .iter()
            .map(|(kind, tail, value)| {
                let path = format!("/apiStages/{api}:{STAGE}/throttle/{tail}");
                match (*kind, value) {
                    ("replace", Some(value)) => replace(&path, value),
                    (kind, value) => op(kind, &path, *value),
                }
            })
            .collect();
        patch_ok(&gateway, case, &format!("/usageplans/{plan}"), operations).await;

        // The READ-BACK, which is what the rule is about: the key the map reports is the
        // method's own reference, whichever spelling the pointer used.
        let read = get_usage_plan(&gateway, &plan).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetUsagePlan, got {} with {:?}",
            read.status, read.body
        );
        let associations = read.body["apiStages"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected apiStages in {:?}", read.body));
        assert_eq!(
            associations.len(),
            1,
            "case {case}: expected the one association the plan was created with, got {:?}",
            read.body
        );
        let reported = &associations[0];
        let expected = expected_throttle(row.throttle);
        match row.throttle.is_empty() {
            // A map with no entry left is the state an association created without one is
            // in, which is the member being absent on the wire.
            true => assert!(
                reported.get("throttle").is_none_or(|held| *held == expected),
                "case {case}: expected no per-method throttle left, got {reported:?}"
            ),
            false => assert_eq!(
                reported["throttle"], expected,
                "case {case}: expected the per-method throttle keyed by the method's own \
                 reference, got {reported:?}"
            ),
        }
        // The association still names the api and the stage it was created for, and the
        // PLAN-level throttle is untouched: a per-method pointer addresses neither.
        assert_eq!(
            reported["apiId"], json!(api),
            "case {case}: expected the association to keep its api, got {reported:?}"
        );
        assert_eq!(
            reported["stage"], json!(STAGE),
            "case {case}: expected the association to keep its stage, got {reported:?}"
        );
        assert_eq!(
            read.body["throttle"],
            json!({ "rateLimit": PLAN_RATE_LIMIT, "burstLimit": PLAN_BURST_LIMIT }),
            "case {case}: expected the plan's own throttle to be untouched, got {:?}",
            read.body
        );
    }
}
