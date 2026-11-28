// SPDX-License-Identifier: CC0-1.0

//! The METADATA CATEGORIES of a domain `CompareDomainConfiguration` compares, and the
//! comparison of two domains in one of them.
//!
//! "Replication copies index data but does not copy domain metadata such as index templates,
//! ingest pipelines, or security roles" (`smithy_query(query="doc",
//! shape="CompareDomainConfiguration")`), so the comparison reads, per category, the entries
//! each domain HOLDS — out of the two stored records and nothing else — and reports how many
//! match, how many exist on only one side, which diverge, and which fields of a diverging
//! entry differ.
//!
//! A domain compared against itself therefore counts every entry in sync and none anywhere
//! else, whatever it holds.

use ::serde_json::{json, Value};

use super::*;

/// What a domain HOLDS in one category, read out of the stored record: one arm per category
/// of [`SUPPORTED_CATEGORIES`].
mod configured;
pub use configured::*;

/// The categories this comparison supports, in the order the refusal of an unsupported one
/// lists them.
///
/// The set is `CompareDomainConfigurationRequest$Categories`' own list
/// (`smithy_query(query="doc",shape="CompareDomainConfigurationRequest",member="Categories")`),
/// which also states that "a category the service does not recognise is rejected with the
/// supported categories named in the error message" — so this list is part of that message's
/// text. Each name is a configuration surface a domain's cluster holds, and the arm below
/// that reads it is [`configured`].
pub const SUPPORTED_CATEGORIES: [&str; 11] = [
    "cluster_settings",
    "security_role_mappings",
    "ism_policies",
    "security_internal_users",
    "aliases",
    "alerting_monitors",
    "templates",
    "component_templates",
    "security_roles",
    "security_action_groups",
    "ingest_pipelines",
];

/// `DiffStatus$compared`: the status a category this call read reports.
pub const DIFF_STATUS_COMPARED: &str = "compared";

/// How many names one category's `Items` carries per list, and how many fields one diverging
/// entry carries: `DiffItemNameList`/`DivergentItemList` are `@length(0, 1000)` and
/// `DivergentFieldList` `@length(0, 100)`, so a category with more differences than that
/// reports the ones it can carry and says so ([`CategoryDiffResult$Truncated`]).
const ITEM_LIST_CAP: usize = 1000;

/// See [`ITEM_LIST_CAP`]: `DivergentFieldList`'s own bound.
const FIELD_LIST_CAP: usize = 100;

/// How long a value a diverging field reports may be: `DiffFieldValue` is
/// `@length(0, 10240)`.
const FIELD_VALUE_CAP: usize = 10_240;

/// How deep into one entry's document the comparison walks before it stops descending.
///
/// An entry's content is a document an earlier call STORED — an index template, an ingest
/// pipeline, a policy — so its nesting is caller-chosen and a walk of it needs a bound that
/// does not depend on the stack: past this depth the subtree is reported as the ONE leaf it
/// sits at, which still tells a caller the two sides differ there and at what path.
const FIELD_DEPTH_CAP: usize = 64;

/// How many leaves of one entry the comparison reads at all.
///
/// The same bound by count that [`FIELD_DEPTH_CAP`] is by depth: a stored document names as
/// many fields as the call that stored it chose, and what the answer can carry is
/// [`FIELD_LIST_CAP`] of them, so nothing is gained by flattening more than this and the work
/// one request asks for stays bounded.
const FIELDS_PER_ENTRY_CAP: usize = 1_000;

/// The category of [`SUPPORTED_CATEGORIES`] that `named` names, or `None` for a value outside
/// the catalogue.
#[must_use]
pub fn supported_category(named: &str) -> Option<&'static str> {
    SUPPORTED_CATEGORIES
        .into_iter()
        .find(|category| *category == named)
}

/// What this call says of a category outside the supported set: the value as sent, and the
/// whole catalogue to choose from.
#[must_use]
pub fn unsupported_category(sent: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Unsupported category: {sent}. Supported: [{}]",
            SUPPORTED_CATEGORIES.join(", ")
        )),
    }
}

/// How the refusal of an unresolvable side names it.
pub const COMPARISON_SOURCE: &str = "Source";

/// See [`COMPARISON_SOURCE`].
pub const COMPARISON_TARGET: &str = "Target";

/// What this call says of a well-formed ARN naming a domain this account does not hold: which
/// side could not be resolved, and the domain its ARN named.
#[must_use]
pub fn comparison_side_not_found(side: &str, domain_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{side} domain {domain_name}: Domain {domain_name} not found."
        )),
    }
}

/// The resource prefix of a domain ARN.
const DOMAIN_RESOURCE: &str = "domain/";

/// The domain a value NAMES, when that value is a domain ARN, and `None` when it is not one at
/// all.
///
/// `DomainArn`'s own pattern admits any partition, service, Region and account
/// (`^arn:aws[a-z\-]*:[a-z]+:[a-z0-9\-]+:[0-9]+:domain\/[a-z0-9\-]+$`), so a comparison takes
/// an ARN of any of them and resolves it against this store afterwards.
#[must_use]
pub fn domain_named_by(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    if !fields.next()?.starts_with("aws") {
        return None;
    }
    // The service, the Region and the account, each unconstrained above.
    let _ = fields.next()?;
    let _ = fields.next()?;
    let _ = fields.next()?;
    let named = fields.next()?.strip_prefix(DOMAIN_RESOURCE)?;
    (!named.is_empty()).then_some(named)
}

/// What one category's comparison reports: the counts, and the names and fields behind them.
#[must_use]
pub fn category_difference(
    source: &BTreeMap<String, Value>,
    target: &BTreeMap<String, Value>,
) -> CategoryDiffResult {
    let source_only: Vec<String> = source
        .keys()
        .filter(|named| !target.contains_key(*named))
        .cloned()
        .collect();
    let target_only: Vec<String> = target
        .keys()
        .filter(|named| !source.contains_key(*named))
        .cloned()
        .collect();
    // An entry both sides hold is either in sync or diverged, decided by its content, so the
    // two counts partition the entries they share.
    let shared = source
        .iter()
        .filter_map(|(named, held)| target.get(named).map(|other| (named, held, other)));
    let mut in_sync = 0usize;
    let mut diverged: Vec<DivergentItem> = Vec::new();
    for (named, held, other) in shared {
        if held == other {
            in_sync += 1;
        } else {
            diverged.push(DivergentItem {
                name: Some(named.clone()),
                fields: Some(divergent_fields(held, other)),
            });
        }
    }
    // "Whether `Items` omits some differences because the category contained more than the
    // response can carry. The counts remain complete when this is true."
    let truncated = source_only.len() > ITEM_LIST_CAP
        || target_only.len() > ITEM_LIST_CAP
        || diverged.len() > ITEM_LIST_CAP
        || diverged
            .iter()
            .any(|item| item.fields.as_ref().is_some_and(|fields| fields.len() > FIELD_LIST_CAP));
    CategoryDiffResult {
        status: Some(DIFF_STATUS_COMPARED.to_string()),
        in_sync: count(in_sync),
        source_only: count(source_only.len()),
        target_only: count(target_only.len()),
        diverged: count(diverged.len()),
        items: Some(DiffItems {
            source_only: Some(capped(source_only)),
            target_only: Some(capped(target_only)),
            diverged: Some(
                capped(diverged)
                    .into_iter()
                    .map(|mut item| {
                        if let Some(fields) = item.fields.take() {
                            item.fields = Some(fields.into_iter().take(FIELD_LIST_CAP).collect());
                        }
                        item
                    })
                    .collect(),
            ),
        }),
        truncated: Some(truncated),
    }
}

/// One count, as the modelled `DiffCount` carries it: a `@range(min: 0)` integer, so a count
/// past what that type holds is reported at its ceiling rather than wrapping.
fn count(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}

/// The first [`ITEM_LIST_CAP`] entries of `named`, the most one `Items` list can carry.
fn capped<T>(named: Vec<T>) -> Vec<T> {
    named.into_iter().take(ITEM_LIST_CAP).collect()
}

/// The fields of one entry both domains hold that differ between them.
///
/// The paths are the entry's own, flattened to dotted text — the form `DiffFieldPath` carries
/// — so a nested document reports the leaf that differs rather than the whole entry. An entry
/// that is not a document at all has one unnamed field: the value itself.
fn divergent_fields(source: &Value, target: &Value) -> Vec<DivergentField> {
    let mut source_fields: BTreeMap<String, Value> = BTreeMap::new();
    flatten(String::new(), source, 0, &mut source_fields);
    let mut target_fields: BTreeMap<String, Value> = BTreeMap::new();
    flatten(String::new(), target, 0, &mut target_fields);
    let mut paths: BTreeSet<&String> = source_fields.keys().collect();
    paths.extend(target_fields.keys());
    paths
        .into_iter()
        .filter(|path| source_fields.get(*path) != target_fields.get(*path))
        .map(|path| DivergentField {
            path: Some(path.clone()),
            source_value: source_fields.get(path).map(field_value),
            target_value: target_fields.get(path).map(field_value),
            // `DiffFieldPolicy` has the one value `redacted`, for a field whose value the
            // comparison withholds. Nothing in these categories is withheld — a credential
            // is no part of what is compared ([`configured`]) — so the member is omitted.
            policy: None,
        })
        .collect()
}

/// One field's value as the text `DiffFieldValue` carries, clipped to what that type holds.
fn field_value(value: &Value) -> String {
    let rendered = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    rendered.chars().take(FIELD_VALUE_CAP).collect()
}

/// Write every LEAF of `value` into `flattened`, under the dotted path it sits at.
///
/// Bounded in both directions a stored document can grow: it descends at most
/// [`FIELD_DEPTH_CAP`] levels — deeper than that the subtree is the leaf — and writes at most
/// [`FIELDS_PER_ENTRY_CAP`] leaves, so neither the nesting nor the width of a document an
/// earlier call stored decides how much stack or memory one comparison takes.
fn flatten(path: String, value: &Value, depth: usize, flattened: &mut BTreeMap<String, Value>) {
    if flattened.len() >= FIELDS_PER_ENTRY_CAP {
        return;
    }
    match value {
        Value::Object(members) if !members.is_empty() && depth < FIELD_DEPTH_CAP => {
            for (name, member) in members {
                let nested = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                flatten(nested, member, depth.saturating_add(1), flattened);
            }
        }
        other => {
            flattened.insert(path, other.clone());
        }
    }
}
