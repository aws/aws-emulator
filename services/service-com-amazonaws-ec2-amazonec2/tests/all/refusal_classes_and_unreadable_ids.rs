// SPDX-License-Identifier: CC0-1.0

//! WHICH CLASS a refusal falls in, driven over the wire the service actually speaks,
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness. EC2 models no error
//! shapes at all, so the wire CODE is the whole contract a client can branch on and a wrong
//! class is a wrong response.
//!
//! Three rules, one per test:
//!
//! 1. an EC2 long-form resource id is an IDENTIFIER the service decodes before it looks
//!    anything up, so a value whose identifier this service issued to a resource of another
//!    kind is no id of the kind the member declares: it is refused with that kind's malformed
//!    class, before any lookup, exactly as a value in another kind's printed prefix is —
//!    while an identifier this service has issued to nothing, and the retired short form,
//!    name a resource that is simply not there;
//! 2. two members that are each acceptable alone but cannot hold together are a parameter
//!    COMBINATION, not a parameter value — Multi-Attach against a volume type that does not
//!    support it, wherever a request can ask for the pair;
//! 3. a value of a MEMBER this service reads without regard to ASCII case is SERVED, so the
//!    request goes on to be validated for everything else — while a member held to its
//!    declared spelling keeps refusing another case, even one whose value set is the very same
//!    one, and a value outside the set is refused whatever its case.
//!
//! Each table spans what its rule generalises over — the resource kind and the operation that
//! reads it, the volume type and the operation that sets it, the spelling and the member — so
//! no row is the specification. Every identifier a row probes is derived from the STORE's own
//! id counter rather than written here, so the rows cannot drift from the mint; every code and
//! sentence asserted is written out as the recorded response gives it.
//!
//! A launch template's `ImageId` of the `resolve:ssm:` form belongs to NO refusal class of
//! this suite: such a value is stored as written and resolved only where a LAUNCH reads it,
//! which is held to its own rule in [`super::launch_template_ssm_alias_forms`].

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LONG_RESOURCE_ID_DIGITS,
    multi_attach_unsupported_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, emulator, error_code, error_message, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The HTTP status every refusal in this suite carries: a fault of the request.
const REFUSAL: u16 = 400;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One row of the id table: what it probes, the operation it probes through, the request's
/// parameters, and the error code due.
type IdRow = (String, &'static str, Vec<(String, String)>, &'static str);

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// `identifier` as the long-form id of the kind `prefix` names — the form
/// `ComAmazonawsEc2Amazonec2State::next_id` mints.
fn long_id(prefix: &str, identifier: u64) -> String {
    format!(
        "{prefix}-{identifier:0width$x}",
        width = LONG_RESOURCE_ID_DIGITS
    )
}

/// The highest identifier this service's shared mint has handed out, read out of the store
/// the handlers read.
fn issued_identifier(engine: &engine::InMemoryEngine) -> u64 {
    engine.state::<Ec2State>().lock().id_counter
}

/// The id of one published AMI the account can see, and the id of the snapshot that AMI's
/// root block device restores from — two identifiers the mint spent on two DIFFERENT kinds.
async fn published_image_and_its_snapshot(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = api(router, "DescribeImages", &[]).await;
    assert_eq!(
        status, 200,
        "the account's published images must be describable, got {status} with body {body}"
    );
    let image_id = elements(&body, "imageId")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the seeded catalog must publish an AMI: {body}"));
    let snapshot_id = elements(&body, "snapshotId")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("a published AMI must restore from a snapshot: {body}"));
    (image_id, snapshot_id)
}

/// Launch one instance from `image_id` and answer its id.
async fn launch(router: &MultiServiceRouter, image_id: &str) -> String {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("the launch must report the instance it minted: {body}"))
}

/// One row of the instance-id table: what the row probes, the id, the code an operation
/// that LOOKS the id up refuses it with, and the code one that DECODES a long-form id first
/// refuses it with.
type InstanceIdRow = (&'static str, String, &'static str, &'static str);

/// RULE 1: a long-form id whose identifier this service spent on another kind of resource is
/// no id of the kind the member declares — that kind's malformed class, before any lookup —
/// while an identifier it has issued to nothing, and the retired short form, are reported as
/// a resource that is not there.
#[tokio::test]
async fn a_long_form_id_the_mint_spent_on_another_kind_is_refused_before_any_lookup() {
    let (router, engine) = emulator();
    let (published_image, its_snapshot) = published_image_and_its_snapshot(&router).await;
    let launched = launch(&router, &published_image).await;

    // The identifier the mint spent on that AMI's own SNAPSHOT, asked for as an id of two
    // other kinds; and an identifier beyond everything the mint has reached, which this
    // service has issued to nothing at all.
    let spent_elsewhere: u64 = its_snapshot
        .rsplit_once('-')
        .and_then(|(_, suffix)| u64::from_str_radix(suffix, 16).ok())
        .unwrap_or_else(|| panic!("a snapshot id carries a hexadecimal identifier: {its_snapshot}"));
    let never_issued = issued_identifier(&engine) + 1_000;

    // (what the row probes, the operation, the parameters, the code due)
    let rows: Vec<IdRow> = vec![
        (
            "an AMI id carrying the identifier of the image's own snapshot".to_string(),
            "DescribeImages",
            indexed("ImageId", &[long_id("ami", spent_elsewhere).as_str()]),
            "InvalidAMIID.Malformed",
        ),
        (
            "an AMI id carrying an identifier the mint has not reached".to_string(),
            "DescribeImages",
            indexed("ImageId", &[long_id("ami", never_issued).as_str()]),
            "InvalidAMIID.NotFound",
        ),
        (
            "the retired short AMI id form, nothing stored under it".to_string(),
            "DescribeImages",
            indexed("ImageId", &["ami-0abc1234"]),
            "InvalidAMIID.NotFound",
        ),
        (
            "another kind's printed prefix as an AMI id".to_string(),
            "DescribeImages",
            indexed("ImageId", &[launched.as_str()]),
            "InvalidAMIID.Malformed",
        ),
    ];
    for (case, action, params, code) in rows {
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, code),
            "case {case:?} on {action}: expected {REFUSAL} {code}, got {status} with body {body}"
        );
    }

    // The same rule, over every operation that reads an INSTANCE id: the identifier the mint
    // spent on another kind is malformed, one it has issued to nothing is not found, and the
    // short form is looked up. `DescribeInstances` DECODES the long form before it looks
    // anything up, so for it an identifier the mint has not reached is malformed too — there
    // is nothing to decode — which is the fourth column.
    let instance_rows: Vec<InstanceIdRow> = vec![
        (
            "the identifier of the published AMI's snapshot",
            long_id("i", spent_elsewhere),
            "InvalidInstanceID.Malformed",
            "InvalidInstanceID.Malformed",
        ),
        (
            "an identifier the mint has not reached",
            long_id("i", never_issued),
            "InvalidInstanceID.NotFound",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the retired short form, nothing launched for it",
            "i-0abc1234".to_string(),
            "InvalidInstanceID.NotFound",
            "InvalidInstanceID.NotFound",
        ),
        (
            "the all-zero placeholder, which no mint counts to",
            "i-00000000000000000".to_string(),
            "InvalidInstanceID.Malformed",
            "InvalidInstanceID.Malformed",
        ),
    ];
    for (case, instance_id, code, decoded) in &instance_rows {
        for action in [
            "DescribeInstances",
            "StopInstances",
            "StartInstances",
            "TerminateInstances",
            "GetPasswordData",
        ] {
            let params = if action == "GetPasswordData" {
                vec![p("InstanceId", instance_id.as_str())]
            } else {
                indexed("InstanceId", &[instance_id.as_str()])
            };
            let expected = if action == "DescribeInstances" {
                *decoded
            } else {
                *code
            };
            let (status, body) = api(&router, action, &params).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (REFUSAL, expected),
                "case {case:?} ({instance_id}) on {action}: expected {REFUSAL} {expected}, got \
                 {status} with body {body}"
            );
        }
    }

    // The operations whose own validator draws NO such distinction answer their own
    // sentence, and one code, for every id that resolves to no instance: the Mac
    // modification task creates discriminate on whether the id names an instance they can
    // act on, so each row above \u2014 whatever made it unresolvable \u2014 is
    // `InvalidInstanceID.Malformed` there.
    for (case, instance_id, _code, _decoded) in &instance_rows {
        let mac_code = "InvalidInstanceID.Malformed";
        let (status, body) = api(
            &router,
            "CreateMacSystemIntegrityProtectionModificationTask",
            &[
                p("InstanceId", instance_id.as_str()),
                p("MacSystemIntegrityProtectionStatus", "disabled"),
            ],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, mac_code),
            "case {case:?} ({instance_id}) on the Mac SIP task: expected {REFUSAL} {mac_code}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The instance ID '{instance_id}' is malformed"),
            "case {case:?}: the Mac SIP task words an instance id it cannot resolve its own \
             way, got body {body}"
        );
    }

    // And the ids that DO resolve still resolve: the refusals above are about the values, not
    // about the operations.
    let (status, body) = api(
        &router,
        "DescribeImages",
        &indexed("ImageId", &[published_image.as_str()]),
    )
    .await;
    assert_eq!(
        (status, element(&body, "imageId").as_deref()),
        (200, Some(published_image.as_str())),
        "the published AMI must still be describable, got {status} with body {body}"
    );
    let (status, body) = api(
        &router,
        "DescribeInstances",
        &indexed("InstanceId", &[launched.as_str()]),
    )
    .await;
    assert_eq!(
        (status, element(&body, "instanceId").as_deref()),
        (200, Some(launched.as_str())),
        "the launched instance must still be describable, got {status} with body {body}"
    );
}

/// RULE 2: Multi-Attach against a volume type that does not support it is a parameter
/// COMBINATION — each member is acceptable alone — wherever a request can ask for the pair,
/// and the sentence names the parameter as the wire spells it and the type that cannot carry
/// it.
#[tokio::test]
async fn multi_attach_on_a_type_that_does_not_support_it_is_a_parameter_combination() {
    let (router, _engine) = emulator();
    let zone = ec2::state::availability_zones_in_region(REGION)[0]
        .zone_name
        .clone();

    // Every volume type the catalog publishes, with the IOPS its type requires: the ones that
    // support Multi-Attach and the ones that do not.
    // Source: `CreateVolumeRequest$VolumeType` / `$MultiAttachEnabled`
    // ("This parameter is supported with io1 and io2 volumes only").
    let rows: [(&str, Option<&str>, bool); 7] = [
        ("io1", Some("100"), true),
        ("io2", Some("100"), true),
        ("gp2", None, false),
        ("gp3", None, false),
        ("st1", None, false),
        ("sc1", None, false),
        ("standard", None, false),
    ];
    for (volume_type, iops, supported) in rows {
        let mut params = vec![
            p("AvailabilityZone", zone.clone()),
            p("Size", "500"),
            p("VolumeType", volume_type),
            p("MultiAttachEnabled", "true"),
        ];
        if let Some(iops) = iops {
            params.push(p("Iops", iops));
        }
        let (status, body) = api(&router, "CreateVolume", &params).await;
        if supported {
            assert_eq!(
                (status, element(&body, "multiAttachEnabled").as_deref()),
                (200, Some("true")),
                "case {volume_type:?}: a type that supports Multi-Attach must accept it, got \
                 {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, "InvalidParameterCombination"),
            "case {volume_type:?}: expected the parameter-combination class, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_message(&body),
            multi_attach_unsupported_message(volume_type),
            "case {volume_type:?}: expected EC2's own sentence for the pair, got body {body}"
        );
    }

    // The same pair reached through the two other operations that can ask for it: a COPY that
    // names the unsupported type, and a MODIFY that moves a Multi-Attach volume onto one.
    let (status, body) = api(
        &router,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone.clone()),
            p("Size", "500"),
            p("VolumeType", "io2"),
            p("Iops", "100"),
            p("MultiAttachEnabled", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the io2 source volume must be created, got {status} with body {body}"
    );
    let source = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("the create must report the volume it minted: {body}"));

    let (status, body) = api(
        &router,
        "CopyVolumes",
        &[
            p("SourceVolumeId", source.as_str()),
            p("AvailabilityZone", zone.clone()),
            p("VolumeType", "gp2"),
            p("MultiAttachEnabled", "true"),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (REFUSAL, "InvalidParameterCombination"),
        "a copy asking for Multi-Attach on gp2 must be refused as a combination, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_message(&body),
        multi_attach_unsupported_message("gp2"),
        "the copy must word the pair the way the create does, got body {body}"
    );

    let (status, body) = api(
        &router,
        "ModifyVolume",
        &[p("VolumeId", source.as_str()), p("VolumeType", "gp3")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (REFUSAL, "InvalidParameterCombination"),
        "a modify onto gp3 of a Multi-Attach volume must be refused as a combination, got \
         {status} with body {body}"
    );
    assert_eq!(
        error_message(&body),
        multi_attach_unsupported_message("gp3"),
        "the modify must word the pair the way the create does, got body {body}"
    );
}

/// RULE 3 (and the parameter-layer half of rule 1): `DescribeVolumes` validates each
/// `VolumeIds` entry as a PARAMETER against what this service can READ as a volume id — the
/// form EC2 issues, plus the decode it makes of a long-form identifier — so a value outside
/// that is `InvalidParameterValue` naming the parameter and the form, while an id it CAN read
/// that the account holds no volume for is the volume's own not-found class.
#[tokio::test]
async fn a_volume_id_outside_the_issued_form_is_a_parameter_level_refusal() {
    let (router, _engine) = emulator();
    let zone = ec2::state::availability_zones_in_region(REGION)[0]
        .zone_name
        .clone();
    let (status, body) = api(
        &router,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone),
            p("Size", "8"),
            p("VolumeType", "gp3"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the volume under test must be created, got {status} with body {body}"
    );
    let held = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("the create must report the volume it minted: {body}"));

    // (what is wrong with the value, the value, the code due)
    let rows: [(&str, &str, &str); 7] = [
        (
            "a leading digit above zero, which the 64-bit identifier never carries",
            "vol-1234567890abcdef0",
            "InvalidParameterValue",
        ),
        (
            "every digit above zero",
            "vol-fffffffffffffffff",
            "InvalidParameterValue",
        ),
        (
            "the all-zero placeholder",
            "vol-00000000000000000",
            "InvalidParameterValue",
        ),
        (
            "a digit that is not hexadecimal",
            "vol-0z23456789abcdef0",
            "InvalidParameterValue",
        ),
        ("another kind's id", "snap-00000000000000001", "InvalidParameterValue"),
        (
            "an identifier inside the issued form that the mint never handed out",
            "vol-00000000000000999",
            "InvalidParameterValue",
        ),
        (
            "the retired short form, nothing created for it",
            "vol-0abc1234",
            "InvalidVolume.NotFound",
        ),
    ];
    for (case, volume_id, code) in rows {
        let (status, body) = api(&router, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, code),
            "case {case:?} ({volume_id}): expected {REFUSAL} {code}, got {status} with body {body}"
        );
        if code == "InvalidParameterValue" {
            assert_eq!(
                error_message(&body),
                format!("Value ({volume_id}) for parameter volumes is invalid. Expected: 'vol-...'."),
                "case {case:?}: expected the parameter-layer sentence, got body {body}"
            );
        }
    }

    // The volume the account does hold is still reported, so the rows above are about the
    // values rather than about the operation.
    let (status, body) = api(&router, "DescribeVolumes", &indexed("VolumeId", &[held.as_str()])).await;
    assert_eq!(
        (status, element(&body, "volumeId").as_deref()),
        (200, Some(held.as_str())),
        "the created volume must still be describable, got {status} with body {body}"
    );
}

/// RULE 4: a value of a MEMBER this service reads without regard to ASCII case is SERVED —
/// the request goes on to be validated for everything else — while a member held to its
/// declared spelling still refuses another case, even one whose value set is the very same
/// one, and a value outside the set is refused whatever case it is sent in.
#[tokio::test]
async fn an_enum_value_ec2_reads_without_regard_to_case_is_served() {
    let (router, _engine) = emulator();

    // The value set the model declares for the Mac SIP status, in every spelling: EC2 reads
    // the member, so the answer is the one the rest of the request earns — here the refusal of
    // an instance id the service cannot read.
    // Source: `MacSystemIntegrityProtectionSettingStatus`, whose catalog is
    // `ec2::ops::types::MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES`.
    let spellings = [
        "disabled", "Disabled", "DISABLED", "enabled", "Enabled", "ENABLED",
    ];
    for spelling in spellings {
        let (status, body) = api(
            &router,
            "CreateMacSystemIntegrityProtectionModificationTask",
            &[
                p("InstanceId", "i-00000000000000000"),
                p("MacSystemIntegrityProtectionStatus", spelling),
            ],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, "InvalidInstanceID.Malformed"),
            "case {spelling:?}: the status must be read as the member it spells, leaving the \
             instance id to be refused, got {status} with body {body}"
        );
    }

    // A value outside the set is still refused, in any case — the set is read, not ignored.
    for outside in ["suspended", "Suspended", "", "disable"] {
        let (status, body) = api(
            &router,
            "CreateMacSystemIntegrityProtectionModificationTask",
            &[
                p("InstanceId", "i-00000000000000000"),
                p("MacSystemIntegrityProtectionStatus", outside),
            ],
        )
        .await;
        assert_eq!(
            status, REFUSAL,
            "case {outside:?}: a value outside the catalog must be refused, got {status} with \
             body {body}"
        );
        assert_ne!(
            error_code(&body),
            "InvalidInstanceID.Malformed",
            "case {outside:?}: a value outside the catalog must be refused for the VALUE, not \
             reach the instance id, got body {body}"
        );
    }

    // A member this service holds to the declared spelling is unaffected, INCLUDING one whose
    // value set is the very same `[disabled, enabled]` — the reading above belongs to the
    // member that was recorded with it, not to a value set a dozen members share.
    let (status, body) = api(
        &router,
        "ModifyInstanceMetadataDefaults",
        &[p("InstanceMetadataTags", "Enabled")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (REFUSAL, "ValidationError"),
        "a member-cased value of a member read by its declared spelling must still be \
         refused, got {status} with body {body}"
    );

    // And one whose value set is another: the same volume answers the declared spelling and
    // refuses the member-cased one, so only the SPELLING can be at fault.
    let zone = ec2::state::availability_zones_in_region(REGION)[0]
        .zone_name
        .clone();
    let (status, body) = api(
        &router,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone),
            p("Size", "8"),
            p("VolumeType", "gp3"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the volume the attribute rows read must be created, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("the create must report the volume it minted: {body}"));
    for (spelling, due) in [("autoEnableIO", 200), ("AutoEnableIO", REFUSAL)] {
        let (status, body) = api(
            &router,
            "DescribeVolumeAttribute",
            &[p("VolumeId", volume_id.as_str()), p("Attribute", spelling)],
        )
        .await;
        assert_eq!(
            status, due,
            "case {spelling:?} on {volume_id}: expected HTTP {due}, got {status} with body \
             {body}"
        );
    }
}
