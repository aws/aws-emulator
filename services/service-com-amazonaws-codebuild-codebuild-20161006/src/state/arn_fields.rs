// SPDX-License-Identifier: CC0-1.0

//! The FIELDS of an ARN an operation that addresses a resource by ARN is handed, and the
//! FIRST one of them that cannot be read.
//!
//! An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and the fields
//! are read in that order: the field a refusal names is the first one that is not well
//! formed, so a value whose region field cannot be a Region name is refused ON THE
//! REGION and the resource behind it is never examined at all. The refusal states which
//! field is at fault and quotes THAT FIELD's value out of the request, which is what
//! tells a caller repairing an ARN which segment to fix — unlike the fixed sentences the
//! resource-kind refusals carry ([`super::INVALID_TOKEN_ARN_MESSAGE`] and the ones
//! beside it), which are reached only once every field before the resource reads.
//!
//! This is one rule for every operation of this service whose input is an ARN, so the
//! field order and the wording cannot drift from one of them to the next.

use super::ARN_NAMESPACE;

/// The fewest hyphen-separated groups a Region name is written with — an area, a
/// direction and a number (`us-east-1`), the shortest form in the AWS General
/// Reference's Region table; a partitioned Region simply carries one group more
/// (`us-gov-west-1`).
const REGION_NAME_GROUPS: usize = 3;

/// Whether `value` is written the way an AWS Region NAME is: at least
/// [`REGION_NAME_GROUPS`] non-empty groups separated by single hyphens, the last of them
/// digits and every earlier one lowercase letters or digits (`us-east-1`,
/// `ap-southeast-4`, `us-gov-west-1`, `us-isob-east-1`).
///
/// The SHAPE question only, the way every other ARN rule of this service answers the
/// shape question ([`super::project_name_from_arn`]): whether this emulator serves that
/// Region is a different question, and an ARN minted in a Region it does not serve names
/// no record it holds rather than a malformed request — which is why a well-formed
/// Region of another partition still resolves to nothing instead of being refused.
pub fn is_region_name(value: &str) -> bool {
    let Some((area, number)) = value.rsplit_once('-') else {
        return false;
    };
    let number_reads = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
    let area_reads = area.split('-').all(|group| {
        !group.is_empty()
            && group
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    });
    number_reads && area_reads && area.split('-').count() + 1 >= REGION_NAME_GROUPS
}

/// The region field of `arn` when that field is the FIRST field of the ARN that cannot
/// be read, or `None` when it reads.
///
/// Every field BEFORE the region has to read for the region to be the first one at
/// fault: the literal `arn`, a non-empty partition and this service's own namespace. The
/// account and the resource fields must be PRESENT — a value with fewer than the six
/// fields of an ARN is no ARN at all, so no single field of it is the one at fault — but
/// they are not read here, because they come after the region.
///
/// Walked field by field rather than collected, so reading an ARN allocates nothing.
fn invalid_arn_region(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != ARN_NAMESPACE {
        return None;
    }
    let region = fields.next()?;
    fields.next()?;
    fields.next()?;
    (!is_region_name(region)).then_some(region)
}

/// What CodeBuild states as the reason the REGION field of an ARN it was handed is not a
/// Region, carried by the modelled `InvalidInputException`.
///
/// Recorded behaviour; the model states no message for the error. The fixed text is
/// `Invalid ARN: region ` and ` is invalid`, and the one variable part is the region
/// field taken verbatim out of the ARN in the request — the field the caller has to
/// repair, which a fixed sentence could not point at.
pub fn invalid_arn_region_message(region: &str) -> String {
    format!("Invalid ARN: region {region} is invalid")
}

/// The sentence an operation addressing a resource by ARN refuses `arn` with when its
/// region field is the first field at fault, or `None` when the refusal belongs to a
/// later field.
///
/// The one call site shape for every such operation: it is asked BEFORE the resource
/// field is read, so an ARN whose region cannot be a Region is never refused for the
/// resource kind behind it.
pub fn arn_region_violation(arn: &str) -> Option<String> {
    invalid_arn_region(arn).map(invalid_arn_region_message)
}
