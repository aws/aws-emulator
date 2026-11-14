// SPDX-License-Identifier: CC0-1.0

//! That the modelled `TagKey` character class is applied CHARACTER BY CHARACTER, over
//! inputs this suite GENERATES rather than enumerates.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes.
//!
//! The rule under test: a modelled `@pattern` holds for every character of the value, so a
//! tag key is admitted only when each of its characters is in one of the general-category
//! groups the declared class names (`\p{L}`, `\p{Z}`, `\p{N}`) or is one of the eight
//! characters it lists literally, and a key carrying any character outside them is refused
//! with `ValidationException` (HTTP 400) — whatever script that character belongs to,
//! wherever in the key it sits, however many of them there are, and whatever other Unicode
//! properties it happens to carry.
//!
//! `tag_key_unicode_class` pins one representative character per general category; this
//! suite is about the quantifier in front of that rule. Its first test walks the combining
//! marks (`M`) of several scripts through every position of a key, beside the same text
//! without the mark and — where Unicode has one — the single precomposed code point for
//! that text, which IS a letter and so IS admitted. Its second builds keys at random from a
//! declared pool of admitted characters and a declared pool of marks, so a character
//! admitted by a property wider than the declared class (`Alphabetic`, say, which combining
//! marks carry through `Other_Alphabetic`) cannot survive by not being one of the rows
//! somebody thought to write down.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, tag_key_refusal_message as expected_refusal, tag_key_router, tag_key_submission,
    tag_keys,
};

/// The tag-key prefix the declared pattern's leading look-around excludes (`smithy_query`
/// shape `TagKey`): a generated key that happens to open with it is refused for that half
/// of the pattern, whatever its characters are.
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Where the submissions below write their output. Each one mints its own invocation, and
/// what this suite reads back is the tag map rather than the output, so one prefix serves
/// the whole suite.
const OUTPUT_PREFIX: &str = "s3://out-bucket/tag-key-marks/";

/// A router serving only this service over one shared engine, with the project every
/// submission below references arranged in the store of the service that owns it.
fn service() -> MultiServiceRouter {
    tag_key_router("tag-key-mark-receipts")
}

/// A submission carrying every required member at its minimum, optionally with `tags`.
fn submission(tags: Option<Value>) -> Value {
    tag_key_submission(OUTPUT_PREFIX, tags)
}

/// Mint one invocation to hang tags on, and answer with its ARN.
async fn invocation(router: &MultiServiceRouter) -> String {
    let answer = call(router, "InvokeDataAutomationAsync", submission(None)).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed submission must be accepted, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .expect("the submission must answer with an ARN")
        .to_string()
}

/// Assert that `key` is refused, with the one wording this service states, by each of the
/// three operations that accept a tag key — the submission that mints a resource
/// (`InvokeDataAutomationAsync`), the one that adds tags (`TagResource`) and the one that
/// removes them (`UntagResource`) — each at the input path its own request carries the key
/// at.
async fn assert_refused(router: &MultiServiceRouter, arn: &str, case: &str, key: &str) {
    for (operation, body, path) in tag_key_requests(arn, key) {
        let answer = call(router, operation, body).await;
        assert_eq!(
            (answer.status, answer.error_code(), answer.message()),
            (400, "ValidationException", expected_refusal(path).as_str()),
            "case {case:?} ({key:?}) on {operation}: the key carries a character outside \
             the modelled TagKey class, expected {:?}, got {} / {}",
            (400, "ValidationException", expected_refusal(path)),
            answer.status,
            answer.body
        );
    }
}

/// Assert that `key` is admitted by the same three operations, and that it really reaches
/// the tag map of `arn` on the way: `TagResource` stores it, `ListTagsForResource` reports
/// it and `UntagResource` takes it away again, leaving the resource as it was found.
async fn assert_admitted(router: &MultiServiceRouter, arn: &str, case: &str, key: &str) {
    let before = tag_keys(router, arn).await;
    for (operation, body, _) in tag_key_requests(arn, key) {
        let answer = call(router, operation, body).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({key:?}) on {operation}: every character is in a group the \
             modelled TagKey class names, got {} / {}",
            answer.status,
            answer.body
        );
        if operation == "TagResource" {
            let mut expected = before.clone();
            if !expected.contains(&key.to_string()) {
                expected.push(key.to_string());
                expected.sort();
            }
            assert_eq!(
                tag_keys(router, arn).await,
                expected,
                "case {case:?} ({key:?}): an admitted key must be stored and reported back"
            );
        }
    }
    assert_eq!(
        tag_keys(router, arn).await,
        before,
        "case {case:?} ({key:?}): removing the key again must leave the resource as it was"
    );
}

/// One request per tag-key-accepting operation carrying `key`, with the input path that
/// operation's request carries it at: the submission and `TagResource` carry whole tags,
/// `UntagResource` carries bare keys. All three are bound by the same `TagKey` shape, so
/// one character cannot be admitted by one and refused by another.
fn tag_key_requests(arn: &str, key: &str) -> [(&'static str, Value, &'static str); 3] {
    [
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
    ]
}

/// One script's combining mark, the base character it attaches to, and the single
/// precomposed code point for that pair where Unicode has one.
struct MarkRow {
    case: &'static str,
    /// Text made only of characters the class names — a letter, here — which is therefore
    /// admitted on its own.
    base: &'static str,
    /// A character in general category `M`, which no group the class names contains.
    mark: char,
    /// The same text as `base` + `mark` as ONE code point, in category `L`: the class
    /// admits that form, which is what makes the refusal of the decomposed form a
    /// statement about characters rather than about text.
    precomposed: Option<&'static str>,
}

/// The marks this suite drives through every position of a key.
///
/// SOURCE of the catalog: the `@pattern` of `smithy_query` shape `TagKey`,
/// `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$`, whose class names the general-category groups
/// `L`, `Z` and `N` and eight literal characters. General category `M` — `Mn`, `Mc`, `Me`
/// — is in none of them, so every character below is outside the class; the scripts are
/// chosen here, and ordinary text in each of them carries such a mark.
const MARK_ROWS: &[MarkRow] = &[
    // Mc, a spacing mark: Devanagari vowel sign AA, as in ordinary Hindi text.
    MarkRow {
        case: "a Devanagari spacing vowel sign (Mc)",
        base: "\u{0915}",
        mark: '\u{093e}',
        precomposed: None,
    },
    // Mn, a nonspacing mark: Devanagari nukta, which Unicode does compose with a letter.
    MarkRow {
        case: "a Devanagari nukta (Mn)",
        base: "\u{0915}",
        mark: '\u{093c}',
        precomposed: Some("\u{0958}"),
    },
    // Mn, a nonspacing mark: Thai vowel sign MAI HAN AKAT, as in ordinary Thai text.
    MarkRow {
        case: "a Thai nonspacing vowel sign (Mn)",
        base: "\u{0e01}",
        mark: '\u{0e31}',
        precomposed: None,
    },
    // Mc, a spacing mark: Bengali vowel sign AA.
    MarkRow {
        case: "a Bengali spacing vowel sign (Mc)",
        base: "\u{0995}",
        mark: '\u{09be}',
        precomposed: None,
    },
    // Mn: an Arabic vowel point.
    MarkRow {
        case: "an Arabic fatha (Mn)",
        base: "\u{062f}",
        mark: '\u{064e}',
        precomposed: None,
    },
    // Mn: a Hebrew point.
    MarkRow {
        case: "a Hebrew hiriq (Mn)",
        base: "\u{05d1}",
        mark: '\u{05b4}',
        precomposed: None,
    },
    // Mn: the Latin combining acute accent, whose composed form is an ordinary letter.
    MarkRow {
        case: "a Latin combining acute accent (Mn)",
        base: "e",
        mark: '\u{0301}',
        precomposed: Some("\u{00e9}"),
    },
    // Me: an enclosing mark, which encloses the character before it.
    MarkRow {
        case: "a combining enclosing circle (Me)",
        base: "x",
        mark: '\u{20dd}',
        precomposed: None,
    },
];

#[tokio::test]
async fn a_combining_mark_is_refused_in_every_script_and_at_every_position() {
    let router = service();
    let arn = invocation(&router).await;

    for MarkRow {
        case,
        base,
        mark,
        precomposed,
    } in MARK_ROWS
    {
        // The control: the same text WITHOUT the mark, and its precomposed single code
        // point where Unicode has one, are made only of characters the class names, so
        // both are admitted and both round-trip.
        assert_admitted(&router, &arn, case, base).await;
        if let Some(precomposed) = precomposed {
            assert_admitted(&router, &arn, case, precomposed).await;
        }

        // The mark itself, at each position a key has: on its own, after its base, before
        // it, in the middle of a longer key, twice over, and among the characters the
        // class lists literally.
        let variants = [
            mark.to_string(),
            format!("{base}{mark}"),
            format!("{mark}{base}"),
            format!("cost{base}{mark}center"),
            format!("{base}{mark}{mark}"),
            format!("a_b.{base}{mark}@z"),
        ];
        for key in &variants {
            assert_refused(&router, &arn, case, key).await;
        }
    }

    // Nothing any refused row carried reached the tag map, and every admitted row removed
    // again what it had added.
    assert_eq!(
        tag_keys(&router, &arn).await,
        Vec::<String>::new(),
        "no key carrying a character outside the modelled TagKey class may be stored"
    );
}

/// Characters the modelled class names, one per group and literal, to generate admitted
/// keys from.
///
/// SOURCE: the class of `smithy_query` shape `TagKey`. `\p{L}` (`Lu`, `Ll`, `Lt`, `Lm`,
/// `Lo`), `\p{Z}` (`Zs`, `Zl`, `Zp`), `\p{N}` (`Nd`, `Nl`, `No`) and the eight literal
/// characters `_ . : / = + - @`.
const ADMITTED_POOL: &[char] = &[
    'A',        // Lu
    'q',        // Ll
    '\u{01c5}', // Lt
    '\u{02b0}', // Lm
    '\u{8acb}', // Lo
    '\u{00a0}', // Zs
    '\u{2028}', // Zl
    '\u{2029}', // Zp
    '7',        // Nd
    '\u{2160}', // Nl
    '\u{00bc}', // No
    '_',
    '.',
    ':',
    '/',
    '=',
    '+',
    '-',
    '@',
];

/// Characters in general category `M`, which the class names nowhere, to generate refused
/// keys from. Every one of them is a mark ordinary text in some script carries, and the
/// first three are `Other_Alphabetic` — so `Alphabetic`, a property strictly wider than
/// the `\p{L}` the class names.
const MARK_POOL: &[char] = &[
    '\u{093e}', // Mc, Devanagari vowel sign AA
    '\u{0e31}', // Mn, Thai vowel sign MAI HAN AKAT
    '\u{09be}', // Mc, Bengali vowel sign AA
    '\u{0301}', // Mn, combining acute accent
    '\u{093c}', // Mn, Devanagari nukta
    '\u{20dd}', // Me, combining enclosing circle
];

/// How many generated keys the property below drives through all three operations. Bounded
/// so the suite stays fast; every case is a full wire round trip.
const GENERATED_CASES: usize = 48;

/// The seed the generated keys come from: fixed, so a failure is reproducible, and printed
/// by the test so a failing run names the case it built.
const SEED: u64 = 0x5bda_7a69_0a11_2026;

#[tokio::test]
async fn generated_keys_are_admitted_exactly_when_every_character_is_in_the_declared_class() {
    let router = service();
    let arn = invocation(&router).await;
    let mut rng = StdRng::seed_from_u64(SEED);
    eprintln!("generated tag-key cases from seed {SEED:#x}");

    for case in 0..GENERATED_CASES {
        // A key of between one and six characters drawn from the admitted pool — the
        // modelled `@length(min: 1, max: 128)` keeps it non-empty and short — with a mark
        // spliced into every second case, at a position the generator chooses so a mark
        // lands at the start, in the middle and at the end across the run.
        let length = rng.random_range(1..=6usize);
        let mut characters: Vec<char> = (0..length)
            .map(|_| ADMITTED_POOL[rng.random_range(0..ADMITTED_POOL.len())])
            .collect();
        let marks = if case % 2 == 0 {
            0
        } else {
            rng.random_range(1..=2usize)
        };
        for _ in 0..marks {
            let mark = MARK_POOL[rng.random_range(0..MARK_POOL.len())];
            let at = rng.random_range(0..=characters.len());
            characters.insert(at, mark);
        }
        let key: String = characters.iter().collect();

        // The rule, stated over the key the generator built: every character has to be in
        // the declared class, and the declared look-around excludes the reserved prefix.
        let admitted = !characters.iter().any(|c| MARK_POOL.contains(c))
            && !key.starts_with(RESERVED_TAG_KEY_PREFIX);
        let label = format!("generated case {case} (seed {SEED:#x}, {marks} mark(s))");
        if admitted {
            assert_admitted(&router, &arn, &label, &key).await;
        } else {
            assert_refused(&router, &arn, &label, &key).await;
        }
    }

    assert_eq!(
        tag_keys(&router, &arn).await,
        Vec::<String>::new(),
        "no generated key carrying a character outside the modelled TagKey class may be \
         stored"
    );
}
