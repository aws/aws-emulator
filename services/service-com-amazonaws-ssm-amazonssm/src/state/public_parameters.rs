// SPDX-License-Identifier: CC0-1.0

//! The AWS-published parameters a read of Parameter Store resolves beside the account's
//! own: the catalog AWS itself maintains in the `/aws` hierarchy of every Region.
//!
//! No caller can write there — `PutParameter` refuses a name prefixed with one of
//! [`super::RESERVED_NAME_PREFIXES`] — so a name in that hierarchy is never the account's
//! and never stored by this service: it is AWS's own published data, and a read resolves it
//! against whatever this emulator holds of that data rather than against the store.
//!
//! What it holds of it today is what the services that publish an image publish about it:
//! every public Systems Manager parameter name of this endpoint, and the value it resolves to
//! (`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_values`) — the ONE place this
//! emulator records what a public parameter resolves to, so Parameter Store and the service
//! that publishes it cannot disagree about it and an id a read reports is an AMI the account
//! can really launch, describe or hand to a service that provisions compute from one. An
//! alias whose image was deregistered resolves to nothing, exactly as the describe reports
//! none.
//!
//! Nothing here writes: the catalog is projected from EC2's image state on each read, so a
//! read of it changes nothing a later caller sees and the values it reports are the values
//! that state holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

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

/// Whether canonical `path` names a NAMESPACE of the reserved hierarchy — one this endpoint
/// serves names in, or a level above one.
///
/// The reserved hierarchy is not an empty tree a caller may walk wherever it likes: it is
/// the namespaces AWS publishes in. `published` is the catalog as this endpoint publishes
/// it, so a path is a namespace when some published name lies at or under it; the reserved
/// Secrets Manager reference namespace counts too, because a read of Parameter Store does
/// serve names from it ([`super::secretsmanager_reference`]), even though no parameter is
/// published there.
///
/// A path anywhere else in the reserved hierarchy names nothing this endpoint has — and
/// nothing the account may have, since no caller can write there — so a read of it is
/// refused ([`invalid_namespace_message`]) rather than answered with an empty page.
pub fn is_reserved_namespace(path: &str, published: &[ParameterRecord]) -> bool {
    let holds = |name: &str| name == path || super::is_under_path(name, path, true);
    holds(super::secretsmanager_reference::REFERENCE_PATH)
        || published
            .iter()
            .any(|record| holds(canonical_name(&record.name)))
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
pub fn published_records<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<ParameterRecord> {
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is no image catalog to publish, so this
        // endpoint publishes nothing rather than inventing a value for a name.
        return Vec::new();
    };
    let store = state.lock();
    store
        .public_ssm_parameter_values(::engine::emulator_account_id())
        .into_iter()
        .map(|(name, published)| {
            let published_at = published
                .published_at
                .as_deref()
                .and_then(parsed_instant)
                // The instant the image was published is the instant its parameter last
                // changed; a date this emulator cannot read reports the epoch rather than
                // now, so a read never reports an instant that moves between two reads.
                .unwrap_or(SystemTime::UNIX_EPOCH);
            published_record(region, &name, &published.value, published_at)
        })
        .collect()
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
