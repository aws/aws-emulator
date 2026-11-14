// SPDX-License-Identifier: CC0-1.0

//! The ARN form a service reads a CALLER-SUPPLIED resource ARN against.
//!
//! An Amazon Resource Name is six colon-separated fields,
//! `arn:<partition>:<vendor>:<region>:<account>:<resource>`, and the resource field
//! may itself carry colons (`…:type:id`) and slashes (`…:type/id`). The third field
//! is the VENDOR — the service namespace, the same value a service publishes as its
//! `aws.api#service$arnNamespace` — and it is what says which service's API can
//! address the named resource at all.
//!
//! Every service whose model binds a `resourceArn` the caller chooses has to read
//! that field: an ARN of another vendor names nothing in this service's namespace,
//! so it is a bad REQUEST (refused on the input, before any lookup) rather than a
//! resource that happens to be missing. The distinction is client-visible — a
//! validation refusal is permanent and caller-side, a not-found is not — so the one
//! parse lives here rather than once per service.
//!
//! `region` and `account` are deliberately allowed to be EMPTY: AWS writes a
//! partition-global or account-less resource that way (`arn:aws:s3:::bucket`), and
//! those ARNs are well formed. The partition, the vendor and the resource are not:
//! an ARN that names no resource, or no vendor, addresses nothing.
//!
//! [`region_of`] is the one reader with a grammar of its own: a service that resolves a
//! caller-supplied ARN against the vendor's endpoint in the ARN's own Region puts that
//! label in a URL, so it is read under the Region grammar the models state rather than
//! as the arbitrary text [`Arn::region`] reports.
//!
//! `service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs` still
//! carries its own copy of this parse (as `arn_vendor`), inlined before this module
//! existed and pinned by that service's own refusal-wording suite; it is to be
//! migrated onto this one rather than copied again.
//!
//! Beside the vendor, a service that reads WHICH resource an ARN names gets the
//! resource field ([`resource`]), the resource TYPE ([`resource_type`]) and the id
//! of a resource of a given type ([`resource_id_of_type`]) from here, so the
//! `<type>/<id>` and `<type>:<id>` forms are read in one place too.
//!
//! [`is_iam_principal_arn`] reads the one ARN class that is not a service's resource at all:
//! an IAM PRINCIPAL — the identity a policy grants to. It spans two vendors and several
//! resource types, so it is a union of forms rather than a [`Requirement`].
//!
//! [`Requirement`] is the form above that: what a member the model DOCUMENTS as the ARN of
//! a particular AWS resource has to name (an IAM role, a Secrets Manager secret, an S3
//! object, an ACM certificate), with the constants for the resources several services bind.
//! A service reads its own model's ARN members against those, so no two services disagree
//! about what "the ARN of an IAM role" is, and keeps only the refusal's envelope to itself.

/// The six fields of a well-formed resource ARN, as a caller submitted them.
///
/// Held as borrows into the value that was parsed, so reading an ARN allocates nothing.
/// `resource` keeps every colon it carries, so an ARN of the `…:type:id` form parses
/// exactly like one of the `…:type/id` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arn<'a> {
    /// The second field (`aws`, `aws-cn`, `aws-us-gov`, …), never empty.
    pub partition: &'a str,
    /// The third field: the service namespace that says whose resource this is, never
    /// empty.
    pub vendor: &'a str,
    /// The fourth field, empty for a partition-global resource.
    pub region: &'a str,
    /// The fifth field, empty for an account-less resource.
    pub account: &'a str,
    /// The sixth field and everything after it, colons included; never empty.
    pub resource: &'a str,
}

impl<'a> Arn<'a> {
    /// The resource TYPE the resource field names: the token before its first `/` or
    /// `:` (`custom-model` of `custom-model/imported/abc123def456`, `execution` of
    /// `execution:machine:run`). The whole resource field when it carries neither.
    #[must_use]
    pub fn resource_type(&self) -> &'a str {
        self.resource
            .split(['/', ':'])
            .next()
            .unwrap_or(self.resource)
    }
}

/// `value`'s six fields, or `None` when `value` is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The ONE parse: a
/// service that needs more than the vendor (the region an ARN names, the resource type
/// it addresses) reads it from here rather than splitting the value again, so no two
/// services can disagree about what an ARN is.
#[must_use]
pub fn parse(value: &str) -> Option<Arn<'_>> {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let vendor = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    (!partition.is_empty() && !vendor.is_empty() && !resource.is_empty()).then_some(Arn {
        partition,
        vendor,
        region,
        account,
        resource,
    })
}

/// How many colon-separated fields an ARN is written with: the literal `arn`, the
/// partition, the vendor, the Region, the account and the resource.
pub const ARN_FIELDS: usize = 6;

/// Whether `value` carries the [`ARN_FIELDS`] colon-separated fields an ARN is written
/// with at all.
///
/// The coarsest question an ARN parse answers, and the only one that can be answered
/// without knowing whose ARN the value would have been: a value with fewer fields has too
/// few separators to be read as an ARN by any service. A service whose refusal for "this
/// is not an ARN" is worded differently from its refusal for "this is an ARN of another
/// vendor" asks this before it asks [`parse_of`]; one that words both the same way needs
/// only [`parse`].
///
/// A value carrying the six fields may still not be a well-formed ARN — [`parse`] also
/// requires the first field to be literally `arn` and the partition, the vendor and the
/// resource to be non-empty.
#[must_use]
pub fn has_arn_fields(value: &str) -> bool {
    // `take` stops the count at six: the resource field may carry further colons, and
    // every field past the sixth belongs to it.
    value.split(':').take(ARN_FIELDS).count() == ARN_FIELDS
}

/// The vendor (service namespace) field of the ARN `value`, or `None` when `value`
/// is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The resource field
/// keeps every colon it carries, so an ARN of the `…:type:id` form parses exactly
/// like one of the `…:type/id` form.
#[must_use]
pub fn vendor(value: &str) -> Option<&str> {
    parse(value).map(|arn| arn.vendor)
}

/// The whole `resource` field of the ARN `value` (`thing/laser-1`,
/// `execution:machine:run`), or `None` when `value` is not a well-formed resource
/// ARN as [`vendor`] defines it.
#[must_use]
pub fn resource(value: &str) -> Option<&str> {
    parse(value).map(|arn| arn.resource)
}

/// The TYPE of resource the ARN `value` names: the `thing` of `thing/laser-1`, the
/// `execution` of `execution:machine:run`, and the whole resource field when it
/// carries no id (`arn:aws:iot:…:job`).
///
/// AWS writes a resource in either form — `<type>/<id>` and `<type>:<id>` — so both
/// separators end the type. `None` for a value that is not a resource ARN, and for
/// one whose resource field begins with a separator and so names no type.
#[must_use]
pub fn resource_type(value: &str) -> Option<&str> {
    let resource_type = parse(value)?.resource_type();
    (!resource_type.is_empty()).then_some(resource_type)
}

/// The ID of the resource the ARN `value` names, when that resource is of type
/// `resource_type` (`thing/laser-1` with `thing` -> `laser-1`); `None` when `value`
/// names another type, or is not a resource ARN.
///
/// The id may be empty (`thing/`): a caller that requires a non-empty id checks it,
/// because "no id" and "another type" are different refusals.
#[must_use]
pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str> {
    let rest = resource(value)?.strip_prefix(resource_type)?;
    rest.strip_prefix('/').or_else(|| rest.strip_prefix(':'))
}

/// Whether `value` is a well-formed resource ARN whose vendor field is `expected`.
///
/// The comparison is exact: an ARN namespace is a fixed lowercase token, so a value
/// differing in case names another vendor.
#[must_use]
pub fn is_resource_arn_of(value: &str, expected: &str) -> bool {
    vendor(value) == Some(expected)
}

/// `value`'s fields when it is a well-formed resource ARN whose vendor is `expected`,
/// the form a service reads its OWN resource ARNs with.
#[must_use]
pub fn parse_of<'a>(value: &'a str, expected: &str) -> Option<Arn<'a>> {
    parse(value).filter(|arn| arn.vendor == expected)
}

/// The longest Region label this parse will report.
///
/// Every `*Arn` pattern in the AWS models bounds the Region field at `[a-z0-9-]{1,20}`,
/// so a longer label is not a Region any AWS endpoint has.
///
/// Public so a suite asserting the boundary of [`region_label`]'s grammar states that
/// bound once, here, rather than restating the figure and drifting from it.
pub const MAX_REGION_LENGTH: usize = 20;

/// `value` when it is a well-formed AWS Region LABEL, and `None` otherwise.
///
/// For a service that has to put a caller-supplied Region into an endpoint URL's
/// AUTHORITY, which is the one place a Region is not just data: a value carrying `/`,
/// `@`, `:`, `?` or `#` would re-point or truncate that authority, so it is reported as
/// no Region at all and the caller falls back to refusing nothing rather than reaching a
/// host the caller chose. The grammar is the one every model's ARN pattern states for the
/// field — non-empty, at most [`MAX_REGION_LENGTH`] characters, composed only of
/// `[a-z0-9-]` — so this is also what [`region_of`] holds an ARN's Region field to.
#[must_use]
pub fn region_label(value: &str) -> Option<&str> {
    let well_formed = !value.is_empty()
        && value.len() <= MAX_REGION_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    well_formed.then_some(value)
}

/// The REGION of `value` when it is a well-formed resource ARN of `expected_vendor` AND
/// that field is a Region LABEL rather than arbitrary text.
///
/// For a service that has to ACT on a caller-supplied ARN's Region — reaching the
/// vendor's endpoint in that Region to resolve the resource the ARN names. A Region read
/// for that is not just data: it ends up in an endpoint URL's authority, so this is
/// stricter than [`Arn::region`], which reports the field as submitted. The label must
/// be non-empty, at most [`MAX_REGION_LENGTH`] characters, and composed only of
/// `[a-z0-9-]` — the grammar every model's ARN pattern states for it — so a value
/// carrying `/`, `@`, `:`, `?` or `#` (which would re-point or truncate an authority) is
/// reported as no Region at all and the caller falls back to its own default.
///
/// `None` therefore means "this value names no Region of that vendor", which covers a
/// value that is not an ARN, an ARN of another vendor, a partition-global ARN whose
/// Region field is empty, and a Region field outside the grammar.
#[must_use]
pub fn region_of<'value>(value: &'value str, expected_vendor: &str) -> Option<&'value str> {
    region_label(parse_of(value, expected_vendor)?.region)
}

/// What an input member a model DOCUMENTS as an ARN of a particular AWS resource has to
/// name.
///
/// Several services bind a member whose documentation says it is "the ARN of" something
/// specific — an IAM role they assume, a Secrets Manager secret they read, an S3 object
/// they fetch — and the models target those members at a bare string with no `@pattern`,
/// so the boundary validator admits any text at all and reading the ARN form is the
/// handler's job. It has to be read: the service RESOLVES what the member names, and a
/// value that is not an ARN of the documented service names nothing, so the request is a
/// parameter violation (attributed to the member) rather than a resource that happens to
/// be missing.
///
/// The requirement is "which vendor, and optionally which resource type", because that is
/// what the documentation fixes; everything else about an ARN (partition, Region, account,
/// the resource name itself) varies per resource and is not the service's business. The
/// six fields come from [`parse_of`], this module's one caller-supplied-ARN parse, so an
/// ARN whose Region and account fields are empty (`arn:aws:s3:::bucket/key`,
/// `arn:aws:iam::000000000000:role/r`) is as well formed here as anywhere else.
///
/// The WORDING of a refusal stays with the service, which owns its error envelope:
/// [`Requirement::phrase`] states the rule as a verb phrase about the member, which each
/// service keys by that member in its own body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// The ARN's third field — the service namespace that says whose resource the value
    /// names (`iam`, `secretsmanager`, `s3`, `acm`).
    pub vendor: &'static str,
    /// The resource type the ARN's resource field must name, for a member whose
    /// documentation names the KIND of resource as well as the service (an IAM *role*, a
    /// Secrets Manager *secret*); `None` where it names only the service.
    pub resource_type: Option<&'static str>,
    /// What a refusal calls the resource this member must name, so the sentence reads as
    /// the member's own documentation does.
    pub label: &'static str,
}

impl Requirement {
    /// Whether `value` is a well-formed resource ARN that satisfies this requirement.
    ///
    /// A value that is not an ARN at all — the empty string included — satisfies nothing,
    /// and the vendor comparison is exact, because an ARN namespace is a fixed lowercase
    /// token.
    #[must_use]
    pub fn admits(self, value: &str) -> bool {
        parse_of(value, self.vendor)
            .is_some_and(|arn| self.resource_type.is_none_or(|kind| arn.resource_type() == kind))
    }

    /// The constraint phrase a member violating this requirement is refused with — a verb
    /// phrase about the member, with no trailing period, so a service can key it by the
    /// member in its own envelope.
    #[must_use]
    pub fn phrase(self) -> String {
        format!("must be the ARN of {}", self.label)
    }
}

/// An IAM role a service assumes: `arn:aws:iam::<account>:role/<name>` — vendor `iam`,
/// resource type `role` — so an ARN of an IAM user or policy names no role to assume.
pub const IAM_ROLE: Requirement = Requirement {
    vendor: "iam",
    resource_type: Some("role"),
    label: "an IAM role",
};

/// A secret in AWS Secrets Manager:
/// `arn:aws:secretsmanager:<region>:<account>:secret:<name>`, so the resource type is
/// `secret`.
pub const SECRETS_MANAGER_SECRET: Requirement = Requirement {
    vendor: "secretsmanager",
    resource_type: Some("secret"),
    label: "a secret in AWS Secrets Manager",
};

/// An object in Amazon S3. Only the vendor is fixed: an S3 object ARN's resource field
/// opens with the BUCKET name (`arn:aws:s3:::bucket/key`), which differs per bucket, so
/// there is no fixed resource type to require.
pub const S3_OBJECT: Requirement = Requirement {
    vendor: "s3",
    resource_type: None,
    label: "an object in Amazon S3",
};

/// A certificate in AWS Certificate Manager:
/// `arn:aws:acm:<region>:<account>:certificate/<id>`, so the resource type is
/// `certificate`.
pub const ACM_CERTIFICATE: Requirement = Requirement {
    vendor: "acm",
    resource_type: Some("certificate"),
    label: "a Certificate Manager certificate",
};

/// The IDENTITY ARN forms, as (the ARN's vendor field, the resource type, the number of
/// `/`-separated name segments that follow it).
///
/// An identity is not a resource of one service, so it is a union rather than a
/// [`Requirement`], which fixes a single vendor and at most one resource type: IAM mints
/// the long-lived identities (the account itself, a user, a role) and STS the temporary
/// ones (an assumed-role session, a federated user), and a policy's principal element
/// names any of them.
///
/// The account root names no further segment. A user, a role and a federated user name
/// one — but a user's and a role's name may be preceded by a PATH
/// (`role/service-role/<name>`), so the count is a minimum. An assumed-role session names
/// the role and the session it opened.
const IDENTITY_RESOURCES: &[(&str, &str, usize)] = &[
    ("iam", "root", 0),
    ("iam", "user", 1),
    ("iam", "role", 1),
    ("sts", "assumed-role", 2),
    ("sts", "federated-user", 1),
];

/// The digits an AWS account id carries: every account id is exactly twelve of them, which
/// is what an identity ARN's account field holds.
const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an AWS ACCOUNT ID: exactly [`ACCOUNT_ID_DIGITS`] decimal digits.
///
/// The one reading of that form, for the account field of an ARN ([`is_iam_principal_arn`])
/// and for a member that carries an account id on its own — a policy's account principal
/// ([`super::iam_policy_principals`]), a registry or owner id — so no two callers disagree
/// about what an account id is.
#[must_use]
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `value` is the ARN of an AWS IAM PRINCIPAL — an identity a policy can grant to.
///
/// For a member a model documents as the ARN of a user or a role, or as a resource-based
/// policy's principal: the models target those at a bare string (often with no more than
/// an `aws.api#arnReference`), so the boundary validator admits any text and reading the
/// identity form is the handler's job. A value outside these forms denotes no identity at
/// all, so it is a parameter violation rather than an identity that happens not to exist.
///
/// Admitted are the five forms [`IDENTITY_RESOURCES`] lists, in any partition. Beyond
/// those, an identity ARN is GLOBAL — IAM and STS write no Region into one — and its
/// account field is the id of the account that owns the identity, which is exactly
/// [`ACCOUNT_ID_DIGITS`] digits. The principal grammar has no partial wildcard either (only
/// the whole-element `"*"`, which is not an ARN), so a value carrying `*` or `?` matches
/// identities rather than naming one, and an identity's path and name are written with `/`
/// alone, so one carrying a `:` is not a name IAM or STS mints.
///
/// A service whose own model states this grammar as a `@pattern` (with its own length
/// bounds on each segment) enforces that pattern instead: this is the form for the members
/// whose models state nothing.
#[must_use]
pub fn is_iam_principal_arn(value: &str) -> bool {
    let Some(arn) = parse(value) else {
        return false;
    };
    let global = arn.region.is_empty();
    let owned_by_an_account = is_account_id(arn.account);
    let names_one_identity = !arn.resource.contains(['*', '?', ':']);
    global
        && owned_by_an_account
        && names_one_identity
        && IDENTITY_RESOURCES
            .iter()
            .any(|(vendor, resource_type, segments)| {
                arn.vendor == *vendor && names_identity(arn.resource, resource_type, *segments)
            })
}

/// Whether the resource field `resource` names an identity of type `resource_type`: the
/// type alone when the form names no segment (the account root), and otherwise the type, a
/// `/`, and at least `segments` non-empty `/`-separated segments.
fn names_identity(resource: &str, resource_type: &str, segments: usize) -> bool {
    let Some(rest) = resource.strip_prefix(resource_type) else {
        return false;
    };
    if segments == 0 {
        return rest.is_empty();
    }
    let Some(named) = rest.strip_prefix('/') else {
        return false;
    };
    let mut named_segments = 0usize;
    for part in named.split('/') {
        if part.is_empty() {
            return false;
        }
        named_segments += 1;
    }
    named_segments >= segments
}

#[cfg(test)]
mod tests {
    use super::{
        has_arn_fields, is_iam_principal_arn, is_resource_arn_of, parse, parse_of, region_label,
        region_of, resource_id_of_type, resource_type, vendor, Requirement, ACM_CERTIFICATE,
        IAM_ROLE, S3_OBJECT, SECRETS_MANAGER_SECRET,
    };

    /// The field count is read on its own: a value carrying the six fields has the
    /// separators an ARN is written with even where the parse rejects it for another
    /// reason, and a value carrying fewer has not.
    #[test]
    fn the_separator_count_is_read_apart_from_the_rest_of_the_parse() {
        // (case, value, whether the six fields are present)
        let rows: &[(&str, &str, bool)] = &[
            (
                "a well-formed ARN",
                "arn:aws:codeartifact:us-east-1:000000000000:domain/d",
                true,
            ),
            (
                "a resource field carrying further colons",
                "arn:aws:states:us-east-1:000000000000:execution:machine:run",
                true,
            ),
            ("an empty region and account", "arn:aws:s3:::bucket/key", true),
            (
                "six fields whose last is empty",
                "arn:aws:codeartifact:us-east-1:000000000000:",
                true,
            ),
            (
                "six fields that do not begin with arn",
                "urn:aws:codeartifact:us-east-1:000000000000:domain/d",
                true,
            ),
            (
                "five fields",
                "arn:aws:codeartifact:us-east-1:000000000000",
                false,
            ),
            ("four fields", "arn:aws:codeartifact:not-an-arn", false),
            ("no separator at all", "codeartifact", false),
            ("the empty string", "", false),
        ];

        for (case, value, expected) in rows {
            let read = has_arn_fields(value);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected has_arn_fields {expected:?} for {value:?}, got {read:?}"
            );
        }
    }

    /// The vendor field is read from the third position, whatever the resource
    /// field's own shape, and only from a value that carries all six fields.
    #[test]
    fn the_vendor_is_the_third_field_of_a_six_field_arn() {
        // (case, value, the vendor the parse must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a slash-form resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                Some("aws-marketplace"),
            ),
            (
                "a colon-form resource carrying further colons",
                "arn:aws:states:us-east-1:000000000000:execution:machine:run",
                Some("states"),
            ),
            (
                "an empty region and account",
                "arn:aws:s3:::bucket/key",
                Some("s3"),
            ),
            (
                "another partition",
                "arn:aws-cn:aws-marketplace:cn-north-1:000000000000:deployment-parameter/dp-1",
                Some("aws-marketplace"),
            ),
            ("not an ARN at all", "dp-0000000000000001", None),
            ("a single character", "-", None),
            ("the empty string", "", None),
            (
                "an ARN that stops before its resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000",
                None,
            ),
            (
                "an ARN with an empty resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000:",
                None,
            ),
            (
                "an ARN with an empty vendor",
                "arn:aws::us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
            (
                "an ARN with an empty partition",
                "arn::aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
            (
                "a value whose first field is not arn",
                "urn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
        ];

        for (case, value, expected) in rows {
            let read = vendor(value);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected vendor {expected:?}, got {read:?}"
            );
        }
    }

    #[test]
    fn a_vendor_match_is_exact() {
        let arn = "arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1";
        // (case, the vendor asked about, whether the ARN is one of its resources)
        let rows: &[(&str, &str, bool)] = &[
            ("its own vendor", "aws-marketplace", true),
            ("another vendor", "secretsmanager", false),
            ("its own vendor in another case", "AWS-Marketplace", false),
            ("a prefix of its vendor", "aws", false),
            ("the empty vendor", "", false),
        ];
        for (case, expected, holds) in rows {
            let answered = is_resource_arn_of(arn, expected);
            assert_eq!(
                answered, *holds,
                "case {case:?}: expected {holds}, got {answered}"
            );
        }
        assert!(
            !is_resource_arn_of("dp-1", "aws-marketplace"),
            "a value that is not an ARN is nobody's resource ARN"
        );
    }

    /// The resource type is what precedes the first separator of the resource
    /// field, in either form AWS writes a resource in, and the id is read back
    /// only for the type that is asked about.
    #[test]
    fn the_resource_type_and_id_are_read_out_of_the_resource_field() {
        // (case, value, the type, the id of a `thing`)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "a slash-form thing",
                "arn:aws:iot:us-east-1:000000000000:thing/laser-1",
                Some("thing"),
                Some("laser-1"),
            ),
            (
                "a colon-form thing",
                "arn:aws:iot:us-east-1:000000000000:thing:laser-1",
                Some("thing"),
                Some("laser-1"),
            ),
            (
                "an id carrying further separators",
                "arn:aws:states:us-east-1:000000000000:execution:machine:run",
                Some("execution"),
                None,
            ),
            (
                "another type",
                "arn:aws:iot:us-east-1:000000000000:job/upgrade-42",
                Some("job"),
                None,
            ),
            (
                "a type that only prefixes the one asked about",
                "arn:aws:iot:us-east-1:000000000000:thinggroup/plant-floor",
                Some("thinggroup"),
                None,
            ),
            (
                "a resource with no id at all",
                "arn:aws:iot:us-east-1:000000000000:job",
                Some("job"),
                None,
            ),
            (
                "a thing with an empty id",
                "arn:aws:iot:us-east-1:000000000000:thing/",
                Some("thing"),
                Some(""),
            ),
            (
                "a resource field that begins with a separator",
                "arn:aws:iot:us-east-1:000000000000:/laser-1",
                None,
                None,
            ),
            ("not an ARN at all", "laser-1", None, None),
            (
                "an ARN with an empty resource",
                "arn:aws:iot:us-east-1:000000000000:",
                None,
                None,
            ),
        ];

        for (case, value, expected_type, expected_id) in rows {
            let read_type = resource_type(value);
            assert_eq!(
                read_type, *expected_type,
                "case {case:?}: expected resource type {expected_type:?}, got {read_type:?}"
            );
            let read_id = resource_id_of_type(value, "thing");
            assert_eq!(
                read_id, *expected_id,
                "case {case:?}: expected thing id {expected_id:?}, got {read_id:?}"
            );
        }
    }

    /// The Region a service may ACT on: read only from an ARN of the vendor asked
    /// about, and only when the field is a Region label rather than arbitrary text —
    /// because the caller puts it in an endpoint URL's authority.
    #[test]
    fn the_region_is_read_only_from_a_well_formed_label() {
        // (case, value, vendor asked about, the Region the parse must report)
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "its own vendor",
                "arn:aws:lambda:us-east-1:000000000000:function:tools",
                "lambda",
                Some("us-east-1"),
            ),
            (
                "a Region of another partition",
                "arn:aws-cn:lambda:cn-north-1:000000000000:function:tools",
                "lambda",
                Some("cn-north-1"),
            ),
            (
                "a GovCloud Region, at the published length",
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:tools",
                "lambda",
                Some("us-gov-west-1"),
            ),
            (
                "another vendor's ARN",
                "arn:aws:s3:us-east-1:000000000000:bucket/key",
                "lambda",
                None,
            ),
            (
                "a partition-global ARN, whose Region field is empty",
                "arn:aws:lambda::000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying a path separator",
                "arn:aws:lambda:us-east-1/evil.example.com:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying userinfo",
                "arn:aws:lambda:us-east-1@evil.example.com:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying a query",
                "arn:aws:lambda:us-east-1?x=1:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field in upper case",
                "arn:aws:lambda:US-EAST-1:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field one character past the published bound",
                "arn:aws:lambda:us-east-1-us-east-1xy:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field exactly at the published bound",
                "arn:aws:lambda:us-east-1-us-east-1x:000000000000:function:tools",
                "lambda",
                Some("us-east-1-us-east-1x"),
            ),
            ("not an ARN at all", "my-function", "lambda", None),
            ("the empty string", "", "lambda", None),
        ];

        for (case, value, expected_vendor, expected) in rows {
            let read = region_of(value, expected_vendor);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected region {expected:?}, got {read:?}"
            );
        }
    }

    /// The same grammar read from a BARE Region, which is what a service that was
    /// handed the Region directly (a request's signing scope) puts in an endpoint
    /// URL's authority.
    #[test]
    fn a_bare_region_is_a_label_only_inside_the_published_grammar() {
        // (case, value, the Region the read must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("a commercial Region", "us-west-2", Some("us-west-2")),
            ("a Region of another partition", "cn-northwest-1", Some("cn-northwest-1")),
            ("a GovCloud Region", "us-gov-east-1", Some("us-gov-east-1")),
            ("a single character", "a", Some("a")),
            ("exactly at the published bound", "us-east-1-us-east-1x", Some("us-east-1-us-east-1x")),
            ("one character past the published bound", "us-east-1-us-east-1xy", None),
            ("a host label appended after a dot", "us-east-1.evil.example.com", None),
            ("a path separator", "us-east-1/evil.example.com", None),
            ("userinfo", "us-east-1@evil.example.com", None),
            ("a port", "us-east-1:8443", None),
            ("a fragment", "us-east-1#x", None),
            ("a query", "us-east-1?x=1", None),
            ("upper case", "US-EAST-1", None),
            ("an underscore", "us_east_1", None),
            ("whitespace", "us-east-1 ", None),
            ("the empty string", "", None),
        ];

        for (case, value, expected) in rows {
            let read = region_label(value);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected region {expected:?}, got {read:?}"
            );
        }
    }

    /// The five fields a row expects, in the order `parse` reads them: partition,
    /// vendor, region, account and the resource TYPE.
    type Fields<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

    /// Every field is read from its own position, and the resource TYPE is the token
    /// before the first `/` or `:` of the resource field whatever follows it.
    #[test]
    fn every_field_is_read_from_its_own_position() {
        // (case, value, (partition, vendor, region, account, resource type))
        let rows: &[(&str, &str, Option<Fields<'_>>)] = &[
            (
                "a slash-form resource",
                "arn:aws:bedrock:us-west-2:000000000000:custom-model/imported/abc123def456",
                Some((
                    "aws",
                    "bedrock",
                    "us-west-2",
                    "000000000000",
                    "custom-model",
                )),
            ),
            (
                "a colon-form resource carrying further colons",
                "arn:aws:states:eu-west-1:000000000000:execution:machine:run",
                Some(("aws", "states", "eu-west-1", "000000000000", "execution")),
            ),
            (
                "an account-less resource in another partition",
                "arn:aws-cn:bedrock:cn-north-1::foundation-model/vendor.model",
                Some(("aws-cn", "bedrock", "cn-north-1", "", "foundation-model")),
            ),
            (
                "a partition-global resource",
                "arn:aws:s3:::bucket",
                Some(("aws", "s3", "", "", "bucket")),
            ),
            ("not an ARN at all", "abc123def456", None),
        ];

        for (case, value, expected) in rows {
            let read = parse(value).map(|arn| {
                (
                    arn.partition,
                    arn.vendor,
                    arn.region,
                    arn.account,
                    arn.resource_type(),
                )
            });
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// `parse_of` is `parse` narrowed to one vendor, with the same exact comparison
    /// `is_resource_arn_of` makes.
    #[test]
    fn parsing_for_one_vendor_answers_only_that_vendors_arns() {
        let arn = "arn:aws:bedrock:us-west-2:000000000000:provisioned-model/abc123def456";
        // (case, the vendor asked about, the region the parse must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("its own vendor", "bedrock", Some("us-west-2")),
            ("another vendor", "kms", None),
            ("its own vendor in another case", "Bedrock", None),
        ];
        for (case, expected, region) in rows {
            let read = parse_of(arn, expected).map(|parsed| parsed.region);
            assert_eq!(
                read, *region,
                "case {case:?}: expected {region:?}, got {read:?}"
            );
        }
        assert_eq!(
            parse_of("provisioned-model/abc123def456", "bedrock"),
            None,
            "a value that is not an ARN is nobody's resource ARN"
        );
    }

    /// A requirement admits exactly the ARNs of its vendor that name its resource type,
    /// whatever the partition, Region, account and resource name are — and admits nothing
    /// that is not a resource ARN.
    #[test]
    fn a_requirement_admits_its_vendors_arns_of_its_resource_type() {
        // (case, the requirement, the value, whether it is admitted)
        let rows: &[(&str, Requirement, &str, bool)] = &[
            (
                "a role ARN, as IAM writes one",
                IAM_ROLE,
                "arn:aws:iam::000000000000:role/mediapackage",
                true,
            ),
            (
                "a role ARN carrying a path",
                IAM_ROLE,
                "arn:aws:iam::000000000000:role/service-role/Harvester",
                true,
            ),
            (
                "a role ARN in the colon form, in another partition",
                IAM_ROLE,
                "arn:aws-cn:iam:cn-north-1:000000000000:role:r",
                true,
            ),
            (
                "an IAM user, which is not a role",
                IAM_ROLE,
                "arn:aws:iam::000000000000:user/bob",
                false,
            ),
            (
                "another vendor's ARN",
                IAM_ROLE,
                "arn:aws:s3:::bucket/key",
                false,
            ),
            (
                "the vendor in another case",
                IAM_ROLE,
                "arn:aws:IAM::000000000000:role/r",
                false,
            ),
            ("not an ARN at all", IAM_ROLE, "my-role", false),
            ("the empty string", IAM_ROLE, "", false),
            (
                "a secret ARN, as Secrets Manager writes one",
                SECRETS_MANAGER_SECRET,
                "arn:aws:secretsmanager:us-east-1:000000000000:secret:cdn-AbCdEf",
                true,
            ),
            (
                "a secret ARN with an empty Region",
                SECRETS_MANAGER_SECRET,
                "arn:aws:secretsmanager::000000000000:secret/cdn",
                true,
            ),
            (
                "another resource type of the same vendor",
                SECRETS_MANAGER_SECRET,
                "arn:aws:secretsmanager:us-east-1:000000000000:rotation/cdn",
                false,
            ),
            (
                "an object ARN, whose resource opens with the bucket",
                S3_OBJECT,
                "arn:aws:s3:::my-bucket/path/to/object",
                true,
            ),
            (
                "a bucket ARN, which the vendor-only requirement also admits",
                S3_OBJECT,
                "arn:aws:s3:::my-bucket",
                true,
            ),
            (
                "another vendor, for a vendor-only requirement",
                S3_OBJECT,
                "arn:aws:s3-outposts:::my-bucket/key",
                false,
            ),
            (
                "a certificate ARN",
                ACM_CERTIFICATE,
                "arn:aws:acm:us-east-1:000000000000:certificate/12345678-1234-1234-1234-123456789012",
                true,
            ),
            (
                "an ACM private-CA ARN, which is not a certificate",
                ACM_CERTIFICATE,
                "arn:aws:acm:us-east-1:000000000000:certificate-authority/ca-1",
                false,
            ),
        ];
        for (case, requirement, value, admitted) in rows {
            let answered = requirement.admits(value);
            assert_eq!(
                answered, *admitted,
                "case {case:?}: expected {admitted}, got {answered} for {value:?}"
            );
        }
    }

    /// The phrase states the rule as a verb phrase about the member, naming the resource
    /// the member's own documentation names.
    #[test]
    fn a_requirements_phrase_names_the_resource_it_requires() {
        // (the requirement, the phrase it states)
        let rows: &[(Requirement, &str)] = &[
            (IAM_ROLE, "must be the ARN of an IAM role"),
            (
                SECRETS_MANAGER_SECRET,
                "must be the ARN of a secret in AWS Secrets Manager",
            ),
            (S3_OBJECT, "must be the ARN of an object in Amazon S3"),
            (
                ACM_CERTIFICATE,
                "must be the ARN of a Certificate Manager certificate",
            ),
        ];
        for (requirement, expected) in rows {
            let phrase = requirement.phrase();
            assert_eq!(
                phrase, *expected,
                "case {:?}: expected {expected:?}, got {phrase:?}",
                requirement.label
            );
        }
    }

    /// An identity ARN is one of the five forms IAM and STS write, in any partition, with
    /// no Region, a numeric account and no wildcard — and nothing else.
    #[test]
    fn an_identity_arn_is_one_of_the_forms_iam_and_sts_write() {
        // (case, value, whether it names an identity)
        let rows: &[(&str, &str, bool)] = &[
            ("the account root", "arn:aws:iam::000000000000:root", true),
            ("an IAM user", "arn:aws:iam::123456789012:user/bob", true),
            (
                "an IAM user under a path",
                "arn:aws:iam::123456789012:user/engineering/bob",
                true,
            ),
            ("an IAM role", "arn:aws:iam::123456789012:role/Agent", true),
            (
                "a role under a path, as a service-linked role is written",
                "arn:aws:iam::123456789012:role/aws-service-role/ops.amazonaws.com/Role",
                true,
            ),
            (
                "an assumed-role session",
                "arn:aws:sts::123456789012:assumed-role/Agent/session",
                true,
            ),
            (
                "a federated user",
                "arn:aws:sts::123456789012:federated-user/Bob",
                true,
            ),
            (
                "an identity in another partition",
                "arn:aws-us-gov:iam::123456789012:role/Agent",
                true,
            ),
            ("not an ARN at all", "not-an-arn", false),
            ("a bare account id", "123456789012", false),
            ("the empty string", "", false),
            ("blank text", "   ", false),
            ("another vendor's resource", "arn:aws:s3:::some-bucket", false),
            (
                "a CodeGuru Profiler resource",
                "arn:aws:codeguru-profiler:us-east-1:123456789012:profilingGroup/pg",
                false,
            ),
            (
                "an IAM resource that is no identity",
                "arn:aws:iam::123456789012:policy/ReadOnly",
                false,
            ),
            (
                "an identity type of the other vendor",
                "arn:aws:iam::123456789012:assumed-role/Agent/session",
                false,
            ),
            (
                "a root the other vendor does not write",
                "arn:aws:sts::123456789012:root",
                false,
            ),
            (
                "a Region, which an identity ARN never carries",
                "arn:aws:iam:us-east-1:123456789012:role/Agent",
                false,
            ),
            ("an empty account", "arn:aws:iam:::role/Agent", false),
            (
                "an account that is not a number",
                "arn:aws:iam::my-account:role/Agent",
                false,
            ),
            (
                "an account id shorter than twelve digits",
                "arn:aws:iam::1:role/Agent",
                false,
            ),
            (
                "an account id longer than twelve digits",
                "arn:aws:iam::0000000000000:role/Agent",
                false,
            ),
            (
                "a colon inside the name, which IAM does not write",
                "arn:aws:iam::123456789012:role/Agent:extra",
                false,
            ),
            (
                "a wildcard in the name",
                "arn:aws:iam::123456789012:role/*",
                false,
            ),
            (
                "a wildcard inside the name",
                "arn:aws:iam::123456789012:role/Agent*",
                false,
            ),
            (
                "a single-character wildcard",
                "arn:aws:iam::123456789012:user/bo?",
                false,
            ),
            ("a bare wildcard", "*", false),
            (
                "a role with no name",
                "arn:aws:iam::123456789012:role/",
                false,
            ),
            (
                "a role type with no separator",
                "arn:aws:iam::123456789012:role",
                false,
            ),
            (
                "a session with no session name",
                "arn:aws:sts::123456789012:assumed-role/Agent",
                false,
            ),
            (
                "a resource type that merely starts like one",
                "arn:aws:iam::123456789012:rootkit",
                false,
            ),
            (
                "the colon form, which IAM does not write for an identity",
                "arn:aws:iam::123456789012:role:Agent",
                false,
            ),
            (
                "the vendor in another case",
                "arn:aws:IAM::123456789012:role/Agent",
                false,
            ),
        ];
        for (case, value, expected) in rows {
            let answered = is_iam_principal_arn(value);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected {expected}, got {answered} for {value:?}"
            );
        }
    }
}
