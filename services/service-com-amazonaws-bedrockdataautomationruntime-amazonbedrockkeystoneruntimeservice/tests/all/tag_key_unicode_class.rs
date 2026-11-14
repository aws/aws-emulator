// SPDX-License-Identifier: CC0-1.0

//! WHICH characters the modelled `TagKey` character class admits, category by category.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes.
//!
//! The rule under test: a modelled `@pattern` is applied exactly as declared, so a tag key
//! is admitted only when every one of its characters is in one of the three Unicode
//! general-category groups the class names (`\p{L}`, `\p{Z}`, `\p{N}`) or is one of the
//! eight characters it lists literally, and a key carrying anything else — including a
//! combining mark (`Mn`, `Mc`, `Me`), which no named group contains — is refused with
//! `ValidationException`. `tag_key_refusal` spans the refusal's wording and the operations
//! that state it; this suite spans the CLASS, one row per general category on each side of
//! it, so a character admitted by a wider approximation of `\p{L}` than the model declares
//! cannot pass unnoticed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, tag_key_refusal_message as expected_refusal, tag_key_router, tag_key_submission,
    tag_keys,
};

/// Where the submissions below write their output. What this suite reads back is the tag
/// map rather than the output, so one prefix serves the whole suite.
const OUTPUT_PREFIX: &str = "s3://out-bucket/tag-key-class/";

/// A router serving only this service over one shared engine, with the project every
/// submission below references arranged in the store of the service that owns it: a
/// submission naming a project nobody created is refused before any rule this suite is
/// about.
fn service() -> MultiServiceRouter {
    tag_key_router("tag-key-class-receipts")
}

/// A submission carrying every required member at its minimum, optionally with `tags`.
fn submission(tags: Option<Value>) -> Value {
    tag_key_submission(OUTPUT_PREFIX, tags)
}

/// One row of the class table: what makes the key interesting, the key, and whether the
/// modelled class admits every character in it.
struct Row {
    case: &'static str,
    key: &'static str,
    admitted: bool,
}

/// The characters the modelled class names, one row per Unicode general category on each
/// side of it.
///
/// SOURCE of the catalog: the `@pattern` of `smithy_query` shape `TagKey`,
/// `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$`. Its class names three general-category groups
/// — `L` (`Lu`, `Ll`, `Lt`, `Lm`, `Lo`), `Z` (`Zs`, `Zl`, `Zp`) and `N` (`Nd`, `Nl`, `No`)
/// — plus eight literal characters, and NOTHING else: every other category (`M`, `P`, `S`,
/// `C`) is outside it, whichever Unicode properties its members also carry. Each row below
/// names the category of the character it turns on; the keys themselves are chosen here,
/// not taken from any one request.
const CLASS_ROWS: &[Row] = &[
    // The five letter categories `\p{L}` covers.
    Row { case: "an uppercase letter (Lu)", key: "OWNER", admitted: true },
    Row { case: "a lowercase letter with a precomposed diacritic (Ll)", key: "ümgebung", admitted: true },
    Row { case: "a titlecase letter (Lt)", key: "\u{01c5}group", admitted: true },
    Row { case: "a modifier letter (Lm)", key: "tier\u{02b0}", admitted: true },
    Row { case: "an other letter (Lo)", key: "請求書", admitted: true },
    // The three separator categories `\p{Z}` covers. No control character is among them,
    // so `White_Space` members like a tab are NOT admitted (a row below).
    Row { case: "a space separator (Zs)", key: "cost\u{a0}center", admitted: true },
    Row { case: "a line separator (Zl)", key: "cost\u{2028}center", admitted: true },
    Row { case: "a paragraph separator (Zp)", key: "cost\u{2029}center", admitted: true },
    // The three number categories `\p{N}` covers.
    Row { case: "a decimal digit (Nd)", key: "stage\u{663}", admitted: true },
    Row { case: "a letter number (Nl)", key: "phase\u{2160}", admitted: true },
    Row { case: "an other number (No)", key: "share\u{bc}", admitted: true },
    // Every character the class lists literally, in one key.
    Row { case: "all eight literal characters", key: "a_b.c:d/e=f+g-h@i", admitted: true },
    // `M` — combining marks. None of the three named groups contains one, so every mark is
    // outside the class, including the ones that carry `Other_Alphabetic` (and so are
    // `Alphabetic`, a property strictly wider than `\p{L}`).
    Row { case: "a spacing combining mark that is Other_Alphabetic (Mc)", key: "\u{0b95}\u{0bbe}", admitted: false },
    Row { case: "a nonspacing combining mark that is Other_Alphabetic (Mn)", key: "\u{0e01}\u{0e31}", admitted: false },
    Row { case: "a nonspacing combining mark that is not Other_Alphabetic (Mn)", key: "e\u{301}nv", admitted: false },
    Row { case: "an enclosing combining mark (Me)", key: "env\u{20dd}", admitted: false },
    // `P` — punctuation outside the eight literals.
    Row { case: "an other punctuation outside the literals (Po)", key: "100%done", admitted: false },
    Row { case: "an open punctuation (Ps)", key: "team[eu]", admitted: false },
    // `S` — symbols.
    Row { case: "a math symbol (Sm)", key: "budget\u{2264}cap", admitted: false },
    Row { case: "a currency symbol (Sc)", key: "budget$", admitted: false },
    Row { case: "an other symbol (So)", key: "env\u{1f680}", admitted: false },
    // `C` — controls and format characters.
    Row { case: "a control character that is White_Space (Cc)", key: "cost\tcenter", admitted: false },
    Row { case: "a format character (Cf)", key: "env\u{200d}ironment", admitted: false },
    // The lookahead half, which excludes exactly the reserved namespace: the same letters
    // without the colon, another case, and the prefix anywhere but the start are ordinary
    // keys.
    Row { case: "the reserved prefix exactly", key: "aws:", admitted: false },
    Row { case: "a key under the reserved prefix", key: "aws:cloudformation:stack-id", admitted: false },
    Row { case: "the reserved letters without the reserved colon", key: "awsome", admitted: true },
    Row { case: "the reserved prefix in another case", key: "AWS:team", admitted: true },
    Row { case: "the reserved prefix away from the start", key: "team:aws:owner", admitted: true },
];

#[tokio::test]
async fn the_modelled_tag_key_class_admits_only_the_categories_it_names() {
    let router = service();
    let answer = call(&router, "InvokeDataAutomationAsync", submission(None)).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed submission must be accepted, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer.body["invocationArn"]
        .as_str()
        .expect("the submission must answer with an ARN")
        .to_string();

    let mut admitted_keys: Vec<String> = Vec::new();

    for Row {
        case,
        key,
        admitted,
    } in CLASS_ROWS
    {
        // One row per operation that accepts a tag key, with the input path that
        // operation's request carries it at: the submission and `TagResource` carry whole
        // tags, `UntagResource` carries bare keys. All three are bound by the same
        // `TagKey` shape, so one character cannot be admitted by one and refused by
        // another.
        let operations: [(&str, Value, &str); 3] = [
            (
                "InvokeDataAutomationAsync",
                submission(Some(json!([{"key": key, "value": "v"}]))),
                "tags.1.member.key",
            ),
            (
                "TagResource",
                json!({"resourceARN": arn, "tags": [{"key": key, "value": "v"}]}),
                "tags.1.member.key",
            ),
            (
                "UntagResource",
                json!({"resourceARN": arn, "tagKeys": [key]}),
                "tagKeys.1.member",
            ),
        ];
        for (operation, body, path) in operations {
            let answer = call(&router, operation, body).await;
            if *admitted {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} ({key:?}) on {operation}: every character is in a group \
                     the modelled TagKey class names, got {} / {}",
                    answer.status,
                    answer.body
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_code(), answer.message()),
                    (400, "ValidationException", expected_refusal(path).as_str()),
                    "case {case:?} ({key:?}) on {operation}: the key carries a character \
                     outside the modelled TagKey class, expected {:?}, got {} / {}",
                    (400, "ValidationException", expected_refusal(path)),
                    answer.status,
                    answer.body
                );
            }
        }
        if *admitted {
            admitted_keys.push((*key).to_string());
        }
    }

    // Nothing a refused row carried reached the tag map, and everything an admitted row
    // carried round-trips: the last `UntagResource` row removed each admitted key again,
    // so adding them all back is exactly what the resource must report.
    let answer = call(
        &router,
        "TagResource",
        json!({
            "resourceARN": arn,
            "tags": admitted_keys
                .iter()
                .map(|key| json!({"key": key, "value": "v"}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "re-adding every admitted key must be accepted, got {} / {}",
        answer.status, answer.body
    );
    let mut expected = admitted_keys.clone();
    expected.sort();
    assert_eq!(
        tag_keys(&router, &arn).await,
        expected,
        "the resource must carry exactly the keys the modelled class admits, and no key \
         carrying a character outside it"
    );
}
