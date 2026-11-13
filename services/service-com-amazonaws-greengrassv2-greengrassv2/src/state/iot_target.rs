// SPDX-License-Identifier: CC0-1.0

//! Does the IoT thing or thing group a deployment TARGETS exist? Put to the plane that
//! holds those records.
//!
//! A deployment's `targetArn` names "the target IoT thing or thing group"
//! (`CreateDeployment$targetArn`), and that record belongs to the AWS IoT control plane,
//! not to this service: nothing here creates a thing, and a copy of one kept beside the
//! deployment store would be a claim about a resource this service does not own. So the
//! question is ASKED of that plane, over the emulator's in-process transport
//! (`OperationContext::callback`), through the reader the IoT model binds for each target
//! form — `DescribeThing` (`GET /things/{thingName}`) and `DescribeThingGroup`
//! (`GET /thing-groups/{thingGroupName}`) — and the answer is what a refusal is grounded
//! in.
//!
//! [`target_is_absent`] answers `true` for ONE case: that plane ANSWERED that it holds no
//! such record (the modelled `ResourceNotFoundException`, HTTP 404). Everything else is a
//! non-answer and refuses nothing — a value that names no IoT thing or thing group at
//! all, a name outside the family's own `ThingName` grammar (which no reader of that
//! plane addresses), an answer this service could not read, and an assembly with no IoT
//! mounted to ask. That asymmetry is deliberate, and the same one
//! `service_common::iam_role` takes for a role: a refusal has to be grounded in what the
//! owning plane said, never in this service's silence.
//!
//! Nothing here reads or writes this service's own store, and the only values that reach
//! the request are the target's resource type and name, the Region (held to the ARN
//! grammar's own label form before it can land in an authority) and the emulator's own
//! credential, which the shared signer puts in the `Authorization` header and nowhere
//! else.

use ::aws_smithy_runtime_api::client::http::HttpConnector;
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::OperationContext;

/// The vendor field an ARN of the IoT control plane carries, and the signing name that
/// plane authenticates a request under.
const IOT: &str = "iot";

/// The resource type a `TargetARN` carries for one device
/// (`^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$`).
const THING: &str = "thing";

/// The resource type that same pattern carries for a group of devices.
const THING_GROUP: &str = "thinggroup";

/// The URI space the IoT model binds its thing readers under (`DescribeThing`:
/// `GET /things/{thingName}`).
const THINGS_SPACE: &str = "/things";

/// The URI space it binds its thing-group readers under (`DescribeThingGroup`:
/// `GET /thing-groups/{thingGroupName}`).
const THING_GROUPS_SPACE: &str = "/thing-groups";

/// The status that plane answers for a name it holds no such record under: both readers
/// model `ResourceNotFoundException` with `httpError` 404.
const NOT_FOUND_STATUS: u16 = 404;

/// The error SHAPE that refusal names, which is what separates it from any other 404 the
/// in-process hop may answer (the router's own `UnknownService`, say, which is what an
/// assembly with no IoT mounted answers).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The IoT record a `TargetARN` names: one device, or a group of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IotTarget {
    /// `thing/<thingName>`: one device.
    Thing(String),
    /// `thinggroup/<thingGroupName>`: every device in that group.
    ThingGroup(String),
}

impl IotTarget {
    /// The target `target_arn` names, or `None` when the value names no IoT thing or
    /// thing group — it is not a resource ARN of `iot`, or its resource field is not one
    /// of the two forms the `TargetARN` pattern admits.
    ///
    /// The ARN itself is read through the crate's one ARN parse
    /// (`service_common::arn`), so this names no ARN grammar of its own.
    #[must_use]
    pub fn from_arn(target_arn: &str) -> Option<Self> {
        let arn = ::service_common::arn::parse_of(target_arn, IOT)?;
        let (resource_type, name) = arn.resource.split_once('/')?;
        if name.is_empty() {
            return None;
        }
        match resource_type {
            THING => Some(Self::Thing(name.to_string())),
            THING_GROUP => Some(Self::ThingGroup(name.to_string())),
            _ => None,
        }
    }

    /// The path of the reader that answers for this target, or `None` when the name is
    /// not one the IoT family's `ThingName` admits
    /// (`service_common::iot_job_executions::is_modeled_thing_name`, the family's one
    /// reading of that shape — `ThingGroupName` declares the same bound and the same
    /// pattern).
    ///
    /// `None` means the question cannot be PUT: no `/things/{thingName}` or
    /// `/thing-groups/{thingGroupName}` request addresses such a name, so nothing is
    /// learned about it and nothing is refused for it. Every name the grammar does admit
    /// is composed of ASCII alphanumerics, `:`, `_` and `-`, each of which stands for
    /// itself in a URI path segment, so the name lands in the path as written.
    fn reader_path(&self) -> Option<String> {
        let (space, name) = match self {
            Self::Thing(name) => (THINGS_SPACE, name),
            Self::ThingGroup(name) => (THING_GROUPS_SPACE, name),
        };
        ::service_common::iot_job_executions::is_modeled_thing_name(name)
            .then(|| format!("{space}/{name}"))
    }
}

/// Whether the IoT control plane ANSWERED that it holds no thing or thing group under the
/// name `target_arn` carries.
///
/// `true` is the one grounded refusal; `false` for every non-answer, as the module header
/// states. `region` is the Region the request was signed for, used only when the ARN
/// names none of its own.
pub async fn target_is_absent(
    ctx: &OperationContext<'_>,
    region: &str,
    target_arn: &str,
) -> bool {
    let Some(path) = IotTarget::from_arn(target_arn).and_then(|target| target.reader_path())
    else {
        return false;
    };
    // The record lives in the Region its own ARN names; a value whose Region field is not
    // a Region label names no endpoint, so the request's Region answers for it, and this
    // service's default answers for an unsigned request.
    let addressed = ::service_common::arn::region_of(target_arn, IOT)
        .or_else(|| ::service_common::arn::region_label(region))
        .unwrap_or(crate::state::DEFAULT_REGION);
    answers_no_such_record(ctx, addressed, &path).await
}

/// Whether `GET path` was answered by the IoT control plane's own
/// `ResourceNotFoundException`: the modelled refusal of a name it holds no record under.
///
/// `false` for every other outcome, which is what keeps a NON-answer from reading as an
/// absent target: the question could not be signed or sent, the answer could not be read,
/// the plane answered something else — and, in an assembly with no IoT mounted at all, the
/// router's own `404 UnknownService`, which is a statement about the services mounted here
/// rather than about the record.
async fn answers_no_such_record(ctx: &OperationContext<'_>, region: &str, path: &str) -> bool {
    let host = format!("{IOT}.{region}.amazonaws.com");
    let Ok(mut request) = ::http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("accept", "application/json")
        .body(Vec::new())
    else {
        return false;
    };
    // The emulator verifies every request signature, so this call carries one too: the
    // credential the framework hands this service, signed through the tree's one
    // in-process signer, under the signing name the IoT control plane authenticates.
    if ::service_common::in_process_sigv4::sign_in_process_request(&mut request, IOT, region, ctx)
        .await
        .is_err()
    {
        return false;
    }
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(parts, SdkBody::from(::bytes::Bytes::from(body)));
    let Ok(request) = ::aws_smithy_runtime_api::http::Request::try_from(request) else {
        return false;
    };
    let connector = ctx.callback().http_connector();
    let Ok(answer) = HttpConnector::call(&connector, request).await else {
        return false;
    };
    answer.status().as_u16() == NOT_FOUND_STATUS
        && named_error_shape(&answer).as_deref() == Some(RESOURCE_NOT_FOUND)
}

/// The error SHAPE a rendered failure names, read from the two places a JSON failure
/// carries it — the header a REST client resolves the shape from, and the body's own
/// discriminator — through the crate's one reading of them
/// (`service_common::error_type_locator`).
///
/// The header may carry the shape followed by `:<url>`, and either place may carry it
/// namespaced; neither the suffix nor the namespace is part of the shape's name, and no
/// SDK reads them as one.
fn named_error_shape(answer: &::aws_smithy_runtime_api::http::Response) -> Option<String> {
    let raw = match answer
        .headers()
        .get(::service_common::error_type_locator::ERROR_TYPE_HEADER)
    {
        Some(header) => header.to_string(),
        None => ::serde_json::from_slice::<::serde_json::Value>(answer.body().bytes()?)
            .ok()?
            .get(::service_common::error_type_locator::SHAPE_DISCRIMINATOR)?
            .as_str()?
            .to_string(),
    };
    let without_suffix = raw.split(':').next().unwrap_or(&raw);
    Some(
        ::service_common::error_type_locator::shape_named(without_suffix).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `TargetARN` names a thing or a thing group of the IoT control plane, and
    /// the reader that answers for it is the one that plane binds for that form.
    ///
    /// The rows differ along what the rule generalises over: both target forms, the
    /// partition, the Region, a name at the punctuation the `ThingName` grammar admits,
    /// a name at its 128-character bound and one past it, a name outside its character
    /// set, and values that name no IoT target at all.
    #[test]
    fn a_target_resolves_to_the_reader_that_answers_for_it() {
        // `ThingName`'s modelled bound, which `ThingGroupName` declares too.
        let longest = "a".repeat(128);
        let too_long = "a".repeat(129);
        let cases = vec![
            (
                "arn:aws:iot:us-east-1:000000000000:thing/MyCore".to_string(),
                Some("/things/MyCore".to_string()),
            ),
            (
                "arn:aws:iot:eu-west-2:000000000000:thinggroup/Fleet".to_string(),
                Some("/thing-groups/Fleet".to_string()),
            ),
            (
                "arn:aws-us-gov:iot:us-gov-west-1:000000000000:thing/Gov_Core-1:2".to_string(),
                Some("/things/Gov_Core-1:2".to_string()),
            ),
            (
                format!("arn:aws:iot:us-east-1:000000000000:thing/{longest}"),
                Some(format!("/things/{longest}")),
            ),
            // Past the grammar's bound, and outside its character set: no reader of that
            // plane addresses such a name, so there is nothing to ask.
            (
                format!("arn:aws:iot:us-east-1:000000000000:thing/{too_long}"),
                None,
            ),
            (
                "arn:aws:iot:us-east-1:000000000000:thinggroup/nested/group".to_string(),
                None,
            ),
            (
                "arn:aws:iot:us-east-1:000000000000:thing/sp ace".to_string(),
                None,
            ),
            // Names no IoT thing or thing group at all.
            (
                "arn:aws:iot:us-east-1:000000000000:certificate/abc".to_string(),
                None,
            ),
            (
                "arn:aws:iam:us-east-1:000000000000:role/MyRole".to_string(),
                None,
            ),
            (
                "arn:aws:iot:us-east-1:000000000000:thing/".to_string(),
                None,
            ),
            ("not-an-arn".to_string(), None),
            (String::new(), None),
        ];
        for (target_arn, expected) in cases {
            let resolved = IotTarget::from_arn(&target_arn).and_then(|target| target.reader_path());
            assert_eq!(
                resolved, expected,
                "case {target_arn:?}: expected the reader path {expected:?}, got {resolved:?}"
            );
        }
    }
}
