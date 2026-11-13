// SPDX-License-Identifier: CC0-1.0

//! The public-access block settings an access point resource carries, driven end
//! to end over HTTP.
//!
//! The rules under test:
//!
//! * an access point resource of this service is blocked from public access by
//!   default, so its four `PublicAccessBlockConfiguration` settings are part of
//!   the resource from the moment it exists — a create that supplies a
//!   configuration leaves exactly it, a create that supplies none leaves all
//!   four settings on — and every read of the resource reports the whole
//!   configuration rather than omitting the member;
//! * a `BlockPublicPolicy` setting in force for an access point — its own or its
//!   account's — refuses a `PutAccessPointPolicy` whose document grants public
//!   access, storing nothing, while `GetAccessPointPolicyStatus` reports that
//!   same derivation over the document a put did store.
//!
//! Every row differs along something those rules generalise over: whether the
//! create request carries the configuration member at all, which of the four
//! settings it spells out inside it (the rest falling to the member's own
//! modelled default), the other members of the create (VPC restriction, a
//! cross-account bucket), which access point resource is created — a
//! single-Region access point or a Multi-Region Access Point, the two resources
//! of this service whose reads report these settings — where the block in force
//! comes from, and how a policy document does or does not open the access point
//! to everyone.
//!
//! The harness is the sibling suite's: the same router, request builder and XML
//! readers a client's request meets.

use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, NO_CONTENT, OTHER_ACCOUNT, REGION, XMLNS,
};

/// The four settings of `PublicAccessBlockConfiguration`, in the order the shape
/// declares them
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`:
/// `BlockPublicAcls`, `IgnorePublicAcls`, `BlockPublicPolicy`,
/// `RestrictPublicBuckets`, each targeting `Setting`).
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// What every setting of an access point resource created WITHOUT a
/// configuration reads back as: blocked.
const BLOCKED_BY_DEFAULT: [bool; 4] = [true, true, true, true];

/// The index of `setting` in [`SETTINGS`].
fn setting_index(setting: &str) -> usize {
    SETTINGS
        .iter()
        .position(|declared| *declared == setting)
        .unwrap_or_else(|| panic!("{setting:?} is not a declared setting of the shape"))
}

/// The settings a create request spells out, or `None` when it carries no
/// configuration member at all.
type Supplied = Option<&'static [(&'static str, bool)]>;

/// The settings a read must report for a resource created with `supplied`.
///
/// A request that supplies no configuration leaves the resource blocked on every
/// setting. A request that DOES supply one leaves exactly what it spelled out,
/// with every setting it left out at that member's own modelled default
/// (`false`), so a configuration naming nothing blocks nothing.
fn expected(supplied: Supplied) -> [bool; 4] {
    match supplied {
        None => BLOCKED_BY_DEFAULT,
        Some(settings) => {
            let mut effective = [false; 4];
            for (setting, value) in settings {
                effective[setting_index(setting)] = *value;
            }
            effective
        }
    }
}

/// The `PublicAccessBlockConfiguration` body member a create request carries, as
/// the element named `wrapper` (the member's name differs between the two
/// resources), or nothing at all when the request supplies no configuration.
fn configuration_xml(wrapper: &str, supplied: Supplied) -> String {
    match supplied {
        None => String::new(),
        Some(settings) => {
            let mut xml = format!("<{wrapper}>");
            for (setting, value) in settings {
                xml.push_str(&format!("<{setting}>{value}</{setting}>"));
            }
            xml.push_str(&format!("</{wrapper}>"));
            xml
        }
    }
}

/// The settings `reply` reports, read from the four elements of the
/// configuration it carries. `None` for a setting the answer omits, which is the
/// failure this suite exists to catch.
fn reported(reply: &Reply) -> Vec<Option<String>> {
    SETTINGS.iter().map(|setting| reply.text(setting)).collect()
}

/// `settings` as the wire spells each boolean.
fn as_wire(settings: [bool; 4]) -> Vec<Option<String>> {
    settings
        .iter()
        .map(|value| Some(value.to_string()))
        .collect()
}

/// One `CreateAccessPoint` request.
struct AccessPointCase {
    case: &'static str,
    name: &'static str,
    bucket: &'static str,
    /// `Some` makes the access point VPC-restricted.
    vpc_id: Option<&'static str>,
    /// The account owning the bucket, for a cross-account access point.
    bucket_account_id: Option<&'static str>,
    supplied: Supplied,
}

impl AccessPointCase {
    const fn new(case: &'static str, name: &'static str, supplied: Supplied) -> Self {
        Self {
            case,
            name,
            bucket: "reports",
            vpc_id: None,
            bucket_account_id: None,
            supplied,
        }
    }

    fn request_body(&self) -> String {
        let mut xml = format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\">");
        xml.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(self.bucket)));
        if let Some(vpc_id) = self.vpc_id {
            xml.push_str(&format!(
                "<VpcConfiguration><VpcId>{}</VpcId></VpcConfiguration>",
                xml_escape(vpc_id)
            ));
        }
        xml.push_str(&configuration_xml(
            "PublicAccessBlockConfiguration",
            self.supplied,
        ));
        if let Some(bucket_account_id) = self.bucket_account_id {
            xml.push_str(&format!(
                "<BucketAccountId>{}</BucketAccountId>",
                xml_escape(bucket_account_id)
            ));
        }
        xml.push_str("</CreateAccessPointRequest>");
        xml
    }
}

/// The access point rows: no row is a renaming of another.
fn access_point_cases() -> Vec<AccessPointCase> {
    vec![
        AccessPointCase::new("no configuration member at all", "default-ap", None),
        AccessPointCase::new(
            "a configuration member spelling out no setting",
            "empty-config-ap",
            Some(&[]),
        ),
        AccessPointCase::new(
            "every setting spelled off",
            "open-ap",
            Some(&[
                ("BlockPublicAcls", false),
                ("IgnorePublicAcls", false),
                ("BlockPublicPolicy", false),
                ("RestrictPublicBuckets", false),
            ]),
        ),
        AccessPointCase::new(
            "every setting spelled on",
            "locked-ap",
            Some(&[
                ("BlockPublicAcls", true),
                ("IgnorePublicAcls", true),
                ("BlockPublicPolicy", true),
                ("RestrictPublicBuckets", true),
            ]),
        ),
        AccessPointCase::new(
            "only BlockPublicPolicy spelled on",
            "policy-blocked-ap",
            Some(&[("BlockPublicPolicy", true)]),
        ),
        AccessPointCase::new(
            "one setting on, one off, two left out",
            "partly-blocked-ap",
            Some(&[("RestrictPublicBuckets", true), ("BlockPublicAcls", false)]),
        ),
        AccessPointCase {
            vpc_id: Some("vpc-0a1b2c3d4e5f67890"),
            ..AccessPointCase::new("no configuration, VPC-restricted", "vpc-default-ap", None)
        },
        AccessPointCase {
            bucket: "partner",
            bucket_account_id: Some(OTHER_ACCOUNT),
            ..AccessPointCase::new(
                "no configuration, cross-account bucket",
                "partner-default-ap",
                None,
            )
        },
        AccessPointCase {
            bucket: "finance",
            ..AccessPointCase::new(
                "the shortest name the model admits, no configuration",
                "abc",
                None,
            )
        },
    ]
}

#[tokio::test]
async fn get_access_point_reports_the_effective_public_access_block_for_every_create_shape() {
    let emulator = Emulator::start();

    for case in access_point_cases() {
        emulator.provision_bucket(case.case, case.bucket);
        let created = emulator
            .call(
                "PUT",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                &case.request_body(),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAccessPoint to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );

        let read = emulator
            .call(
                "GET",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                "",
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetAccessPoint to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        assert_eq!(
            reported(&read),
            as_wire(expected(case.supplied)),
            "case {:?}: expected every setting of PublicAccessBlockConfiguration reported, \
             got body {}",
            case.case,
            read.body
        );

        // A read changes nothing a later reader sees: the configuration is one
        // value per access point, not one per read.
        let again = emulator
            .call(
                "GET",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                "",
            )
            .await;
        assert_eq!(
            reported(&again),
            reported(&read),
            "case {:?}: expected the configuration to be stable across reads, got body {}",
            case.case,
            again.body
        );
    }
}

/// One `PublicAccessBlock` URI: the account-level configuration both the write
/// and the read of this suite's account block are bound to.
const ACCOUNT_BLOCK_URI: &str = "/v20180820/configuration/publicAccessBlock";

/// Where the `BlockPublicPolicy` setting that applies to an access point comes
/// from. The most restrictive of the settings that apply is in force, so each
/// source blocks on its own and both together block too.
#[derive(Debug, Clone, Copy)]
enum BlockedFrom {
    /// Neither the access point nor its account blocks public policies.
    Nowhere,
    /// The access point's own configuration blocks them.
    AccessPoint,
    /// The owning account's configuration blocks them.
    Account,
    /// Both do.
    Both,
}

impl BlockedFrom {
    fn access_point_blocks(self) -> bool {
        matches!(self, Self::AccessPoint | Self::Both)
    }

    fn account_blocks(self) -> bool {
        matches!(self, Self::Account | Self::Both)
    }

    fn blocks(self) -> bool {
        !matches!(self, Self::Nowhere)
    }
}

/// One policy document, with whether it grants public access.
struct PolicyCase {
    case: &'static str,
    document: &'static str,
    public: bool,
}

/// The policy rows: documents on both sides of the publicness derivation, each
/// differing in HOW it does or does not open the access point to everyone.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "a bare wildcard principal",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: true,
        },
        PolicyCase {
            case: "a wildcard AWS principal",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: true,
        },
        PolicyCase {
            case: "a wildcard among named principals",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","*"]},"Action":"s3:*","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: true,
        },
        PolicyCase {
            case: "an allow that names who it excludes rather than who it admits",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotPrincipal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: true,
        },
        PolicyCase {
            case: "a single statement written as an object rather than a list",
            document: r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}}"#,
            public: true,
        },
        PolicyCase {
            case: "a principal scoped to one account",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: false,
        },
        PolicyCase {
            case: "a wildcard principal on a deny",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#,
            public: false,
        },
        PolicyCase {
            case: "a wildcard principal narrowed to the requester's account",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*","Condition":{"StringEquals":{"aws:PrincipalAccount":"123456789012"}}}]}"#,
            public: false,
        },
        PolicyCase {
            case: "a wildcard principal narrowed to one VPC",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*","Condition":{"StringEquals":{"aws:SourceVpc":"vpc-0a1b2c3d4e5f67890"}}}]}"#,
            public: false,
        },
        PolicyCase {
            case: "a document that grants nothing to anyone",
            document: r#"{"Version":"2012-10-17","Statement":[]}"#,
            public: false,
        },
    ]
}

/// Create an access point whose own `BlockPublicPolicy` is `blocks` and whose
/// other three settings are spelled off, so a refusal below can only come from
/// that one setting.
async fn create_with_block_public_policy(
    emulator: &Emulator,
    account: &str,
    name: &str,
    blocks: bool,
) -> Reply {
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(name, "reports");
    let body = format!(
        "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
         <PublicAccessBlockConfiguration><BlockPublicAcls>false</BlockPublicAcls>\
         <IgnorePublicAcls>false</IgnorePublicAcls><BlockPublicPolicy>{blocks}</BlockPublicPolicy>\
         <RestrictPublicBuckets>false</RestrictPublicBuckets>\
         </PublicAccessBlockConfiguration></CreateAccessPointRequest>"
    );
    emulator
        .call("PUT", &format!("/v20180820/accesspoint/{name}"), account, &body)
        .await
}

async fn put_policy(emulator: &Emulator, account: &str, name: &str, document: &str) -> Reply {
    let body = format!(
        "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessPointPolicyRequest>",
        xml_escape(document)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            &body,
        )
        .await
}

async fn get_policy(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            "",
        )
        .await
}

async fn get_policy_status(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policyStatus"),
            account,
            "",
        )
        .await
}

#[tokio::test]
async fn a_public_policy_is_refused_exactly_where_block_public_policy_is_in_force() {
    for source in [
        BlockedFrom::Nowhere,
        BlockedFrom::AccessPoint,
        BlockedFrom::Account,
        BlockedFrom::Both,
    ] {
        let emulator = Emulator::start();
        if source.account_blocks() {
            let put = emulator
                .call(
                    "PUT",
                    ACCOUNT_BLOCK_URI,
                    ACCOUNT,
                    &format!(
                        "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">\
                         <BlockPublicPolicy>true</BlockPublicPolicy>\
                         </PublicAccessBlockConfiguration>"
                    ),
                )
                .await;
            assert_eq!(
                put.status, NO_CONTENT,
                "case {source:?}: expected PutPublicAccessBlock to answer {NO_CONTENT}, got {} \
                 with body {}",
                put.status, put.body
            );
        }

        for (row, policy) in policy_cases().iter().enumerate() {
            let name = format!("block-{row}-ap");
            let created =
                create_with_block_public_policy(&emulator, ACCOUNT, &name, source.access_point_blocks())
                    .await;
            assert_eq!(
                created.status, 200,
                "case {source:?}/{:?}: expected CreateAccessPoint to answer 200, got {} with body {}",
                policy.case, created.status, created.body
            );

            let refused = source.blocks() && policy.public;
            let put = put_policy(&emulator, ACCOUNT, &name, policy.document).await;
            if refused {
                assert_eq!(
                    (put.status, put.code().as_str()),
                    (403, "AccessDenied"),
                    "case {source:?}/{:?}: expected the public document refused, got {} with body {}",
                    policy.case,
                    put.status,
                    put.body
                );
                assert_eq!(
                    put.text("Message").as_deref(),
                    Some("Access Denied"),
                    "case {source:?}/{:?}: expected the service's refusal message, got body {}",
                    policy.case,
                    put.body
                );
                // Nothing was stored, so the access point still carries no
                // policy at all.
                let read = get_policy(&emulator, ACCOUNT, &name).await;
                assert_eq!(
                    (read.status, read.code().as_str()),
                    (404, "NoSuchAccessPointPolicy"),
                    "case {source:?}/{:?}: expected no policy stored, got {} with body {}",
                    policy.case,
                    read.status,
                    read.body
                );
            } else {
                assert_eq!(
                    put.status, NO_CONTENT,
                    "case {source:?}/{:?}: expected the document stored, got {} with body {}",
                    policy.case, put.status, put.body
                );
                let read = get_policy(&emulator, ACCOUNT, &name).await;
                assert_eq!(
                    read.text("Policy").as_deref(),
                    Some(policy.document),
                    "case {source:?}/{:?}: expected the document back byte for byte, got body {}",
                    policy.case,
                    read.body
                );
            }
        }
    }
}

#[tokio::test]
async fn policy_status_reports_whether_the_stored_policy_grants_public_access() {
    let emulator = Emulator::start();

    for (row, policy) in policy_cases().iter().enumerate() {
        let name = format!("status-{row}-ap");
        // Nothing blocked, so every document below is stored and its publicness
        // is observable rather than refused.
        let created = create_with_block_public_policy(&emulator, ACCOUNT, &name, false).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAccessPoint to answer 200, got {} with body {}",
            policy.case, created.status, created.body
        );

        // No policy yet: a policy status is a statement ABOUT a policy, so the
        // answer is the missing-policy refusal rather than a status.
        let before = get_policy_status(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            (before.status, before.code().as_str()),
            (404, "NoSuchAccessPointPolicy"),
            "case {:?}: expected the missing-policy refusal, got {} with body {}",
            policy.case,
            before.status,
            before.body
        );
        assert_eq!(
            before.text("Message").as_deref(),
            Some("The specified accesspoint policy does not exist"),
            "case {:?}: expected the service's missing-policy message, got body {}",
            policy.case,
            before.body
        );

        assert_eq!(
            put_policy(&emulator, ACCOUNT, &name, policy.document)
                .await
                .status,
            NO_CONTENT,
            "case {:?}: expected the document stored",
            policy.case
        );
        let status = get_policy_status(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            status.status, 200,
            "case {:?}: expected GetAccessPointPolicyStatus to answer 200, got {} with body {}",
            policy.case, status.status, status.body
        );
        assert_eq!(
            status.text("IsPublic").as_deref(),
            Some(if policy.public { "true" } else { "false" }),
            "case {:?}: expected IsPublic derived from the stored document, got body {}",
            policy.case,
            status.body
        );
    }

    // An access point the account does not own has no policy status either.
    let absent = get_policy_status(&emulator, ACCOUNT, "no-such-ap").await;
    assert_eq!(
        (absent.status, absent.code().as_str()),
        (404, "NoSuchAccessPoint"),
        "expected the missing access point refused, got {} with body {}",
        absent.status,
        absent.body
    );
}

/// One `CreateMultiRegionAccessPoint` request.
struct MultiRegionCase {
    case: &'static str,
    name: &'static str,
    client_token: &'static str,
    supplied: Supplied,
}

impl MultiRegionCase {
    fn request_body(&self) -> String {
        format!(
            "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
             <ClientToken>{token}</ClientToken><Details><Name>{name}</Name>{block}\
             <Regions><Region><Bucket>reports</Bucket></Region></Regions>\
             </Details></CreateMultiRegionAccessPointRequest>",
            token = xml_escape(self.client_token),
            name = xml_escape(self.name),
            block = configuration_xml("PublicAccessBlock", self.supplied),
        )
    }
}

/// The Multi-Region Access Point rows, spanning the same class of inputs.
fn multi_region_cases() -> Vec<MultiRegionCase> {
    vec![
        MultiRegionCase {
            case: "no configuration member at all",
            name: "default-mrap",
            client_token: "t-default",
            supplied: None,
        },
        MultiRegionCase {
            case: "a configuration member spelling out no setting",
            name: "empty-config-mrap",
            client_token: "t-empty",
            supplied: Some(&[]),
        },
        MultiRegionCase {
            case: "every setting spelled on",
            name: "locked-mrap",
            client_token: "t-locked",
            supplied: Some(&[
                ("BlockPublicAcls", true),
                ("IgnorePublicAcls", true),
                ("BlockPublicPolicy", true),
                ("RestrictPublicBuckets", true),
            ]),
        },
        MultiRegionCase {
            case: "only IgnorePublicAcls spelled on",
            name: "acls-ignored-mrap",
            client_token: "t-acls",
            supplied: Some(&[("IgnorePublicAcls", true)]),
        },
    ]
}

#[tokio::test]
async fn get_multi_region_access_point_reports_the_effective_public_access_block() {
    let emulator = Emulator::start();

    for case in multi_region_cases() {
        emulator.provision_bucket(case.case, "reports");
        let created = call_in_region(
            &emulator.router,
            REGION,
            "POST",
            "/v20180820/async-requests/mrap/create",
            ACCOUNT,
            &case.request_body(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateMultiRegionAccessPoint to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );

        let read = call_in_region(
            &emulator.router,
            REGION,
            "GET",
            &format!("/v20180820/mrap/instances/{}", case.name),
            ACCOUNT,
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetMultiRegionAccessPoint to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        assert_eq!(
            reported(&read),
            as_wire(expected(case.supplied)),
            "case {:?}: expected every setting of the reported PublicAccessBlock, got body {}",
            case.case,
            read.body
        );
    }
}
