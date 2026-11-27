// SPDX-License-Identifier: CC0-1.0

//! A census of the `@pattern` declarations the model snapshot spells with a SECOND
//! escaping level, and what this service does about each.
//!
//! A pattern whose expression carries `\\` where the published expression carries `\`
//! asks for a literal backslash: the framework's boundary compiles it and applies it
//! verbatim (`vendor/smithy-schema-server/src/constraints.rs`), so every value of the
//! documented form is refused before a handler runs. The snapshot spells thirteen of
//! its expressions that way and thirty-two of them as published, so this is a property
//! of individual shapes rather than of the model as a whole — and the census below is
//! what keeps the list a REFRESHED snapshot brings from growing unnoticed.
//!
//! Two of the thirteen reach the input of an operation this crate serves, and those
//! two carry `skip_validation` with their operations applying the published expression
//! instead (`tests/all/phone_number_syntax.rs`). The other eleven reach only inputs of
//! operations that are still scaffolds answering HTTP 501, so no request of theirs is
//! ever judged by the expression; each is the business of the issue that implements its
//! operation, which is where the same treatment belongs.

use serde_json::Value;

/// The published model snapshot this crate is generated from, embedded at compile time
/// so the census is taken from the model rather than from a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-connect-amazonconnectservice.json"
));

/// The shapes whose expression this service applies itself, because an operation it
/// serves takes one as input. Spelled without the namespace, as the census reports
/// them.
const APPLIED_BY_THIS_SERVICE: [&str; 2] = ["PhoneNumber", "PhoneNumberPrefix"];

/// The shapes whose expression no handler judges yet: every operation whose input
/// reaches one answers HTTP 501 (`grep -l 'SCAFFOLDED, NOT IMPLEMENTED' src/ops` names
/// them — the data-table, prompt, theme and workspace planes).
const REACHED_ONLY_BY_SCAFFOLDS: [&str; 11] = [
    "DataTableDescription",
    "MediaSource",
    "Page",
    "S3Uri",
    "Slug",
    "ThemeImageLink",
    "ThemeString",
    "WorkspaceAssociatedResourceName",
    "WorkspaceDescription",
    "WorkspaceName",
    "WorkspaceTitle",
];

/// Every shape in `shapes` whose own `@pattern` carries a second escaping level, by
/// short name.
fn over_escaped(model: &Value) -> Vec<String> {
    let mut found: Vec<String> = model["shapes"]
        .as_object()
        .expect("the snapshot carries a shapes object")
        .iter()
        .filter_map(|(shape_id, shape)| {
            let pattern = shape["traits"]["smithy.api#pattern"].as_str()?;
            // A backslash followed by a backslash: the escape of an escape, which is
            // what makes the expression ask for a literal backslash.
            pattern.contains("\\\\").then(|| {
                shape_id
                    .rsplit('#')
                    .next()
                    .unwrap_or(shape_id.as_str())
                    .to_string()
            })
        })
        .collect();
    found.sort();
    found
}

#[test]
fn every_over_escaped_expression_is_one_this_crate_has_accounted_for() {
    let model: Value = serde_json::from_str(SNAPSHOT).expect("the model snapshot parses");
    let found = over_escaped(&model);

    let mut accounted: Vec<String> = APPLIED_BY_THIS_SERVICE
        .iter()
        .chain(REACHED_ONLY_BY_SCAFFOLDS.iter())
        .map(|shape| (*shape).to_string())
        .collect();
    accounted.sort();

    assert_eq!(
        found, accounted,
        "the snapshot's over-escaped expressions must be the ones this crate accounts \
         for: a shape it adds needs the treatment `src/state/phone_number_syntax.rs` \
         describes, and one it drops should give way to the request boundary; found \
         {found:?}, accounted for {accounted:?}"
    );
}

#[test]
fn an_expression_spelled_as_published_is_not_counted_as_over_escaped() {
    // The discriminator itself, over the two spellings the snapshot carries for ONE
    // expression — the E.164 number — so the census cannot be reading something else:
    // `SensitivePhoneNumber` spells it as published and `PhoneNumber` spells it with
    // the second escaping level.
    let model: Value = serde_json::from_str(SNAPSHOT).expect("the model snapshot parses");
    let found = over_escaped(&model);
    for (case, shape, over_escaped_here) in [
        ("the published spelling", "SensitivePhoneNumber", false),
        ("the doubled spelling", "PhoneNumber", true),
    ] {
        let counted = found.iter().any(|name| name == shape);
        assert_eq!(
            counted, over_escaped_here,
            "case {case:?} ({shape}): expected counted={over_escaped_here}, got {counted}"
        );
    }
}
