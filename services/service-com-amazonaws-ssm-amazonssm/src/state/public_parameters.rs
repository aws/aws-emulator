// SPDX-License-Identifier: CC0-1.0

//! The AWS-published parameters a read of Parameter Store resolves beside the account's
//! own: the catalog AWS itself maintains in the `/aws` hierarchy of every Region.
//!
//! No caller can write there — `PutParameter` refuses a name prefixed with one of
//! [`super::RESERVED_NAME_PREFIXES`] — so a name in that hierarchy is never the account's
//! and never stored by this service: it is AWS's own published data, and a read resolves it
//! against whatever this emulator holds of that data rather than against the store.
//!
//! What it holds of it today is three catalogues, read as one. The Global Infrastructure
//! reference data AWS publishes in every Region ([`super::global_infrastructure`]) — the
//! Regions, their long names, partitions and endpoint domains — is the same at every
//! endpoint and is held here. Beside it is what the services that publish a RESOURCE publish
//! about it, each read from the service that publishes it rather than restated here: every
//! public Systems Manager parameter name of this endpoint, and the value it resolves to
//! (`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_values` for the images AWS
//! publishes, `published_layer_parameters` for the layer versions AWS Lambda publishes) —
//! the ONE place this emulator records what such a parameter resolves to, so Parameter Store
//! and the service that publishes it cannot disagree about it and the identifier a read
//! reports names a resource that service really serves: an AMI the account can launch,
//! describe or hand to a service that provisions compute from one, and a layer version a
//! function can mount. An alias whose image was deregistered resolves to nothing, exactly as
//! the describe reports none.
//!
//! Nothing here writes: the catalog is projected from EC2's image state on each read, so a
//! read of it changes nothing a later caller sees and the values it reports are the values
//! that state holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use ::service_com_amazonaws_lambda_awsgirapiservice::state as lambda;

use super::{
    canonical_name, ParameterRecord, ParameterVersionRecord, DEFAULT_DATA_TYPE, TIER_STANDARD,
    TYPE_STRING,
};

/// The top hierarchy level AWS publishes its own parameters in, canonical (no leading
/// `/`): the `aws` level, which is the one `PutParameter` refuses to write into
/// ([`super::RESERVED_NAME_PREFIXES`]), so everything in it is AWS's and nothing in it is
/// the account's.
pub const PUBLISHED_ROOT: &str = "aws";

/// How many versions of a published name this emulator holds: AWS republishes such a
/// parameter whenever it publishes a new build, and this catalog holds the one build its
/// image catalog publishes, which is that parameter's first and only version.
pub const PUBLISHED_VERSION: i64 = 1;

/// Whether `name` lies in AWS's published hierarchy, and is therefore resolved against the
/// catalog rather than against the account's own parameters. Takes either spelling of the
/// name (with or without its leading `/`).
pub fn is_published_name(name: &str) -> bool {
    let canonical = canonical_name(name);
    canonical == PUBLISHED_ROOT
        || canonical
            .strip_prefix(PUBLISHED_ROOT)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a hierarchy read of canonical `path` reads AWS's published catalog.
///
/// A hierarchy read answers from ONE of the two: the published catalog for a path inside
/// AWS's own hierarchy, and the account's parameters for every other path. Nothing is lost
/// either way, because neither hierarchy can hold a parameter of the other — and AWS's
/// catalog is reached by naming it, not by listing the root of an account that owns none of
/// it.
pub fn is_published_hierarchy(path: &str) -> bool {
    is_published_name(path)
}

/// Whether canonical `path` lies in a NAMESPACE of the reserved hierarchy — one this
/// endpoint serves names in, a level above one, or a level inside one.
///
/// The reserved hierarchy is not an empty tree a caller may walk wherever it likes: it is
/// the namespaces AWS publishes in. A namespace is the hierarchy a served name lies in
/// ([`namespaces`]), and a read reaches it from either side: the levels ABOVE it, which hold
/// it, and the levels INSIDE it, which it holds — a deeper level of a namespace that exists
/// reports the nothing it holds at that level, exactly as a hierarchy of the account's own
/// parameters that holds nothing does.
///
/// A path in NO namespace of the reserved hierarchy names nothing this endpoint has — and
/// nothing the account may have, since no caller can write there — so a read of it is
/// refused ([`invalid_namespace_message`]) rather than answered with an empty page.
pub fn is_reserved_namespace(path: &str, published: &[ParameterRecord]) -> bool {
    namespaces(published).iter().any(|namespace| {
        *namespace == path
            // The path holds the namespace: a level above it, which is read to discover it.
            || super::is_under_path(namespace, path, true)
            // The namespace holds the path: a level inside it, which it may hold nothing at.
            || super::is_under_path(path, namespace, true)
    })
}

/// Whether canonical `path`, read as a HIERARCHY, leads toward a namespace of the reserved
/// hierarchy at all: whether the PUBLISHER level it lies under bears one
/// ([`publisher_bears_a_namespace`]).
///
/// The reserved root itself lies under no publisher level and holds every one of them, so it
/// leads toward them all. A path that leads toward a namespace is resolved against the
/// namespaces themselves ([`is_reserved_namespace`]), so one naming none of them is refused
/// as denied; a path that leads toward none names no hierarchy of that tree at all, so what
/// is left of it is a parameter NAME carrying the reserved root, which the name grammar
/// refuses ([`super::PARAMETER_NAME_REQUIREMENTS`] states that rule: a name cannot be
/// prefixed with the reserved roots).
pub fn leads_to_reserved_namespace(path: &str, published: &[ParameterRecord]) -> bool {
    publisher_level(path).is_none_or(|level| publisher_bears_a_namespace(&level, published))
}

/// Whether canonical `name` lies in a hierarchy AWS PUBLISHES in: whether the publisher
/// level it lies under bears a namespace this endpoint serves names in.
///
/// A name is not a hierarchy, so the reserved root alone lies in none: it is a name carrying
/// the reserved root and nothing more. What this answers is whether a call addressed to the
/// name addresses AWS's own published data ([`reserved_name_refusal_message`]).
pub fn names_published_data(name: &str, published: &[ParameterRecord]) -> bool {
    publisher_level(name).is_some_and(|level| publisher_bears_a_namespace(&level, published))
}

/// What a call addressed to `name` in the reserved hierarchy is refused with — the one
/// statement of that sentence, for every call of this service that addresses a name there.
///
/// Which sentence it is turns on WHERE in the reserved hierarchy the name lies, not on which
/// call asked: a name in a hierarchy AWS publishes in ([`names_published_data`]) addresses
/// AWS's own published data, which is not the account's to read as its own or to change
/// ([`super::RESTRICTED_PARAMETER_MESSAGE`]), while a name under a publisher level AWS
/// publishes nothing under addresses no data at all — it is a parameter name carrying the
/// reserved root, which no caller may create or reach
/// ([`super::reserved_name_denial_message`]).
pub fn reserved_name_refusal_message(
    engine: &(impl Send + Sync + 'static),
    region: &str,
    name: &str,
) -> String {
    let published = published_records(engine, region);
    if names_published_data(canonical_name(name), &published) {
        super::RESTRICTED_PARAMETER_MESSAGE.to_string()
    } else {
        super::reserved_name_denial_message(name)
    }
}

/// What a READ addressed to `name` in the reserved hierarchy is refused with, or `None` when
/// the hierarchy SERVES that name and the read resolves it.
///
/// The reserved hierarchy is gated for a read exactly as it is for a write: a name in it is
/// never the account's, so a caller may address only what this endpoint publishes there — the
/// namespaces AWS publishes in and the reserved namespace another service's secrets are
/// served through ([`namespaces`]). A name under a publisher level that bears NONE of them
/// addresses no data at all, so it is refused as the reserved NAME it is
/// ([`super::reserved_name_denial_message`]) rather than reported as a name nothing is stored
/// under: the refusal is about the namespace being closed, not about the name being absent,
/// so a read of it discloses no more than a write of it does.
///
/// A name the hierarchy does serve a read of is resolved — against the published catalog
/// ([`published`]) or against the service the reserved reference namespace reads
/// ([`super::secretsmanager_reference`]) — and a name inside a served namespace that nothing
/// is published under is the ordinary miss, exactly as a hierarchy read of such a level
/// reports the nothing it holds ([`is_reserved_namespace`]).
pub fn reserved_read_denial_message(
    engine: &(impl Send + Sync + 'static),
    region: &str,
    name: &str,
) -> Option<String> {
    if !is_published_name(name) {
        return None;
    }
    let published = published_records(engine, region);
    (!names_published_data(canonical_name(name), &published))
        .then(|| super::reserved_name_denial_message(name))
}

/// The publisher level canonical `path` lies under — the two levels `<reserved
/// root>/<publisher>` of it — or `None` when it is the reserved root itself, which lies
/// under none.
///
/// Below the reserved root that level names the publisher of the namespaces under it (what
/// AWS's services publish, and the secrets this service serves through the hierarchy
/// reserved for them); it is part of the reserved tree's shape rather than of any one
/// namespace.
fn publisher_level(path: &str) -> Option<String> {
    let mut levels = path.splitn(3, '/');
    let root = levels.next().unwrap_or_default();
    let publisher = levels.next()?;
    Some(format!("{root}/{publisher}"))
}

/// Whether the publisher level `level` bears a namespace this endpoint serves names in: one
/// AT it or anywhere under it.
fn publisher_bears_a_namespace(level: &str, published: &[ParameterRecord]) -> bool {
    namespaces(published)
        .iter()
        .any(|namespace| *namespace == level || super::is_under_path(namespace, level, true))
}

/// The namespaces of the reserved hierarchy this endpoint serves names in, canonical.
///
/// A published name is a name IN a namespace, so the namespace is the hierarchy it lies in —
/// "the hierarchy is the parameter name except the last part of the parameter"
/// (`GetParametersByPathRequest$Path`) — and `published` is the catalog as this endpoint
/// publishes it, so the set is read off the catalog rather than written down beside it. The
/// reserved Secrets Manager reference namespace is one too, because a read of Parameter Store
/// serves names from it ([`super::secretsmanager_reference`]) even though no parameter is
/// published there; it is the namespace ITSELF, not the hierarchy of a name, because the
/// names it serves are the secrets held under it.
fn namespaces(published: &[ParameterRecord]) -> Vec<&str> {
    let mut namespaces = vec![super::secretsmanager_reference::REFERENCE_PATH];
    for record in published {
        let name = canonical_name(&record.name);
        let hierarchy = name.rsplit_once('/').map_or(name, |(above, _leaf)| above);
        if !namespaces.contains(&hierarchy) {
            namespaces.push(hierarchy);
        }
    }
    namespaces
}

/// What a hierarchy read of a path in the reserved hierarchy that names no namespace of it
/// ([`is_reserved_namespace`]) is refused with.
///
/// The sentence names the reserved hierarchy and the path that is not one of its
/// namespaces, as the path reads canonically — without the leading separator, which is how
/// a parameter name's store key spells it ([`canonical_name`]).
pub fn invalid_namespace_message(path: &str) -> String {
    format!(
        "No access to \"/{PUBLISHED_ROOT}/\" namespace: {path} is not a valid namespace"
    )
}

/// Every parameter AWS publishes at this endpoint, in ascending canonical-name order — the
/// order a hierarchy read's resume cursor is total in.
///
/// Three catalogues, read as one: the Global Infrastructure reference data AWS publishes in
/// every Region ([`super::global_infrastructure`]), which is the same at every endpoint and
/// is held here, the names the services that publish an image publish about it, which are
/// projected from that service's own image state, and the names AWS Lambda publishes the ARN
/// of a layer version of its own under, projected from that service's own published-layer
/// catalog.
pub fn published_records<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<ParameterRecord> {
    let mut records: Vec<ParameterRecord> = super::global_infrastructure::published_names()
        .into_iter()
        .map(|(name, value)| {
            published_record(
                region,
                &name,
                &value,
                super::global_infrastructure::PUBLISHED_AT,
            )
        })
        .collect();
    if let Some(state) = ec2::service_state(engine) {
        let store = state.lock();
        records.extend(
            store
                .public_ssm_parameter_values(::engine::emulator_account_id())
                .into_iter()
                .map(|(name, published)| {
                    let published_at = published
                        .published_at
                        .as_deref()
                        .and_then(parsed_instant)
                        // The instant the image was published is the instant its parameter
                        // last changed; a date this emulator cannot read reports the epoch
                        // rather than now, so a read never reports an instant that moves
                        // between two reads.
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    published_record(region, &name, &published.value, published_at)
                }),
        );
    }
    // The layer versions AWS Lambda publishes into every Region, each advertised under the
    // public name a client resolves its ARN by: the value is read from that service's own
    // catalog, so the ARN this read reports is the one that service's own reads answer for
    // and a function's `Layers` accepts.
    records.extend(
        lambda::published_layer_parameters(region)
            .into_iter()
            .map(|(name, value)| published_record(region, &name, &value, lambda::PUBLISHED_AT)),
    );
    // One ascending order over every catalogue, which is what makes a hierarchy read's
    // resume cursor total: no name can be in two of them, since none publishes in another's
    // hierarchy.
    records.sort_by(|left, right| canonical_name(&left.name).cmp(canonical_name(&right.name)));
    records
}

/// The parameter AWS publishes under `name` at this endpoint, or `None` for a name AWS
/// publishes nothing under.
pub fn published(
    engine: &(impl Send + Sync + 'static),
    region: &str,
    name: &str,
) -> Option<ParameterRecord> {
    if !is_published_name(name) {
        return None;
    }
    let wanted = canonical_name(name);
    published_records(engine, region)
        .into_iter()
        .find(|record| canonical_name(&record.name) == wanted)
}

/// The parameters AWS publishes among `names`, keyed by canonical name — the batch form of
/// [`published`], so a read of many names projects the catalog once.
pub fn published_among(
    engine: &(impl Send + Sync + 'static),
    region: &str,
    names: &[&str],
) -> BTreeMap<String, ParameterRecord> {
    if !names.iter().any(|name| is_published_name(name)) {
        return BTreeMap::new();
    }
    published_records(engine, region)
        .into_iter()
        .filter(|record| {
            names
                .iter()
                .any(|name| canonical_name(name) == canonical_name(&record.name))
        })
        .map(|record| (canonical_name(&record.name).to_string(), record))
        .collect()
}

/// Whether a metadata listing narrowed by `filters` is ADDRESSED AT AWS's published
/// hierarchy, and so resolves the published catalog beside the account's own parameters.
///
/// Each filter is the `(key, option, values)` triple a listing applies. A listing is
/// addressed at the published hierarchy when one of its filters CONFINES it to that
/// hierarchy — a `Path` filter whose value lies in it, or a `Name` filter anchored at a name
/// in it (`Equals` or `BeginsWith`, the two options that anchor a name; `Contains` searches
/// for a fragment anywhere in a name and so confines a listing to nothing).
///
/// A listing that is not confined to it reports the account's own parameters alone: AWS
/// publishes its catalog in every account and Region alike and no account owns any of it,
/// so an unnarrowed "list the parameters in your account" is not a request for AWS's.
pub fn addressed_by_filters(filters: &[(&str, &str, &[String])]) -> bool {
    filters.iter().any(|(key, option, values)| match *key {
        super::FILTER_KEY_PATH => values.iter().any(|value| is_published_hierarchy(value)),
        super::FILTER_KEY_NAME if *option != super::FILTER_OPTION_CONTAINS => {
            values.iter().any(|value| is_published_name(value))
        }
        _ => false,
    })
}

/// One published parameter, as a record the reads project the same way they project a
/// stored one — so a published parameter is reported through [`ParameterRecord`]'s own
/// `to_parameter` rather than through a second rendering that could disagree with it.
///
/// It is a `String` parameter of the standard tier carrying `text` data, with the single
/// version [`PUBLISHED_VERSION`] and no label: AWS publishes a value, not a hierarchy of
/// labelled revisions of it, and nothing here holds an earlier build of the name. It
/// carries no tag, because a published parameter is not the account's resource to tag.
fn published_record(
    region: &str,
    name: &str,
    value: &str,
    published_at: SystemTime,
) -> ParameterRecord {
    ParameterRecord {
        name: name.to_string(),
        region: region.to_string(),
        tags: BTreeMap::new(),
        versions: vec![ParameterVersionRecord {
            version: PUBLISHED_VERSION,
            value: value.to_string(),
            type_: TYPE_STRING.to_string(),
            data_type: DEFAULT_DATA_TYPE.to_string(),
            tier: TIER_STANDARD.to_string(),
            description: None,
            allowed_pattern: None,
            key_id: None,
            policies: None,
            last_modified_date: published_at,
            // AWS publishes this parameter and no account can write it, so no user of
            // this account last changed it and none is reported as having done so.
            last_modified_user: None,
            labels: Default::default(),
        }],
    }
}

/// `text` as an instant, or `None` when it is no timestamp this emulator reads.
///
/// The published catalog's dates are rendered the way AWS renders an image's
/// `CreationDate`, which is RFC 3339 in UTC — the `DateTime` wire form.
fn parsed_instant(text: &str) -> Option<SystemTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(parsed).ok()
}
