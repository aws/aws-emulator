// SPDX-License-Identifier: CC0-1.0

//! What a LOGGING trail delivers on the emulator a customer STARTS, for an account served
//! OUTSIDE the partition's leader Region: the binary on its own port, reached over TLS with
//! SigV4-signed calls whose credential scope names that Region.
//!
//! RULE: a trail homed in the Region a client works in delivers that Region's calls into its
//! bucket under the documented key layout — `<key-prefix>/AWSLogs/<account>/CloudTrail/
//! <region>/YYYY/MM/DD/`, each file named
//! `<account>_CloudTrail_<region>_<minute>_<unique>.json.gz`, with no key prefix at all when
//! the trail carries none — and which records it delivers is decided by its own event
//! selectors: a management call reaches a trail whose selectors log management events, and a
//! call on the CONTENTS of a resource reaches a trail only while one of its advanced event
//! selectors admits that record, down to the ARN of the resource the call acted on. The Region
//! is not part of the rule: the leader Region is where the partition's GLOBAL services record,
//! which is a fact about those services' events, so a trail homed anywhere delivers its own
//! Region's calls exactly as one homed there does. The sibling suite
//! `cloudtrail_cognito_identity_trail_delivery_on_a_started_emulator` pins the same rule in the
//! leader Region with a selector that names a resource TYPE; this one adds the Region a client
//! is not in, a trail carrying NO key prefix, data-plane calls made WITH a credential, and a
//! selector narrowed to one pool's ARN.
//!
//! The instrument is two trails of one account, so an absence is the selector in force
//! withholding a record and never a record that landed elsewhere:
//!
//! * the MANAGEMENT trail carries NO key prefix and the selectors a create gives it ("trails
//!   created without specific event selectors will be configured to log all read and write
//!   management events, and no data events"), so the account's control-plane calls land at the
//!   bucket's own root layout and no data event can;
//! * the DATA trail delivers under a prefix of its own and carries one advanced event selector
//!   naming the `Data` category, the identity-pool resource type and the ARN of ONE of the two
//!   pools, so only that pool's identity flow can land under it.
//!
//! What a delivered FILE is (a gzip stream whose JSON holds its records under `Records`) and
//! what each RECORD carries are pinned where the gzip reader lives — the CloudTrail crate's
//! `tests/all/trail_delivery_in_the_region_a_trail_is_homed_in.rs` and
//! `tests/all/trail_log_file_delivery_of_data_events.rs`. `core/router/Cargo.toml` is a pinned
//! path in this repository, so this test crate can take no decompressor: here a delivered file
//! is asserted by its key layout and by WHICH trail's prefix it arrived under, which is what
//! the selector decides.
//!
//! The started binary is driven through the harness this test crate publishes for exactly that
//! ([`crate::cfn_deployed_pipeline_on_a_started_emulator::Started`]), whose calls take the
//! Region they are signed for.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::Started;
use crate::query_wire::elements;

/// The Region this account is served in: NOT the partition's leader Region, so a single-Region
/// trail that logs no global-service events covers these calls by being homed where they are
/// served and by nothing else.
const REGION: &str = "us-west-2";

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which owns
/// the bucket, the pools and the trails and names every delivered log file's prefix.
const ACCOUNT: &str = "000000000000";

/// The one bucket both trails deliver into, and the prefix the DATA trail delivers under —
/// the management trail carries none at all.
const BUCKET: &str = "identity-pool-delivery-outside-the-leader-region";
const DATA_PREFIX: &str = "data";

/// The two trails.
const MANAGEMENT_TRAIL: &str = "identity-pool-management-trail";
const DATA_TRAIL: &str = "identity-pool-data-trail";

/// The `X-Amz-Target` prefix of each service called below, and the `awsJson1_1` content type
/// all three speak.
const CLOUDTRAIL_TARGET_PREFIX: &str = "CloudTrail_20131101.";
const IDENTITY_TARGET_PREFIX: &str = "AWSCognitoIdentityService.";
const USER_POOLS_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService.";
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The `eventCategory` an advanced event selector pins a data event by, and the resource type
/// an identity pool is named by in a record's `resources` and in a selector's `resources.type`.
const DATA_CATEGORY: &str = "Data";
const IDENTITY_POOL_TYPE: &str = "AWS::Cognito::IdentityPool";

/// The segments of a CloudTrail log-file key, and what such a file's name ends with.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The user the fixture user pool signs in, and the password it signs in with.
const POOL_USER: &str = "delivery-user";
const POOL_PASSWORD: &str = "Delivery-Passw0rd!1";

/// How long a delivery is waited for, and how often it is looked for. The wait is on a
/// CONDITION and the span is generous, so a loaded host spends none of it; what it bounds is a
/// delivery that never comes.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// How many times an ABSENCE is looked for before it is believed. A delivery is made while the
/// call that caused it is being served — the positive legs below find theirs on the first
/// look — so a prefix that is still empty after this many looks is empty because the selector
/// in force withheld the record.
const ABSENCE_LOOKS: usize = 10;

/// The emulator this suite starts, with the calls it makes on it — all of them signed for
/// [`REGION`].
struct Account<'a> {
    started: &'a Started,
}

impl Account<'_> {
    /// One SIGNED `awsJson1_1` call that has to be served.
    fn served(&self, signing_name: &str, target: &str, operation: &str, body: &Value) -> Value {
        let (status, text) = self.started.rpc_in_region(
            REGION,
            signing_name,
            &format!("{target}{operation}"),
            JSON_CONTENT_TYPE,
            body,
        );
        assert_eq!(
            status,
            200,
            "{signing_name} {operation} must answer 200, got {status} with {text}; the log:\n{}",
            self.started.log()
        );
        if text.is_empty() {
            return Value::Null;
        }
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    /// One CloudTrail call that has to be served.
    fn cloudtrail(&self, operation: &str, body: &Value) -> Value {
        self.served("cloudtrail", CLOUDTRAIL_TARGET_PREFIX, operation, body)
    }

    /// One Cognito Identity call — the identity-pool administration a client makes with AWS
    /// developer credentials, and the identity flow an application makes with the credentials
    /// it holds.
    fn identity(&self, operation: &str, body: &Value) -> Value {
        self.served("cognito-identity", IDENTITY_TARGET_PREFIX, operation, body)
    }

    /// One Amazon Cognito user pools call.
    fn user_pools(&self, operation: &str, body: &Value) -> Value {
        self.served("cognito-idp", USER_POOLS_TARGET_PREFIX, operation, body)
    }

    /// One Amazon S3 call of this Region, as its status and body.
    fn s3(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, String) {
        self.started
            .call_in_region(REGION, "s3", method, path, headers, body)
    }

    /// The one bucket both trails deliver into: created in this Region, carrying a tag set and
    /// the policy CloudTrail's own documentation states — the ACL check on the bucket and the
    /// object write under the area each trail delivers into, which is narrower than a grant on
    /// the whole bucket.
    fn log_bucket(&self) {
        let configuration = format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        );
        let (status, text) = self.s3(
            "PUT",
            &format!("/{BUCKET}"),
            &[("content-type", "application/xml")],
            configuration.into_bytes(),
        );
        assert_eq!(
            status, 200,
            "CreateBucket({BUCKET:?}) must succeed, got {status} with {text}"
        );
        let (status, text) = self.s3(
            "PUT",
            &format!("/{BUCKET}?tagging"),
            &[("content-type", "application/xml")],
            "<Tagging><TagSet><Tag><Key>purpose</Key><Value>trail-logs</Value></Tag></TagSet>\
             </Tagging>"
                .as_bytes()
                .to_vec(),
        );
        assert!(
            (200..300).contains(&status),
            "PutBucketTagging({BUCKET:?}) must succeed, got {status} with {text}"
        );
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Sid": "AWSCloudTrailAclCheck",
                    "Effect": "Allow",
                    "Principal": { "Service": "cloudtrail.amazonaws.com" },
                    "Action": "s3:GetBucketAcl",
                    "Resource": format!("arn:aws:s3:::{BUCKET}"),
                },
                {
                    "Sid": "AWSCloudTrailWrite",
                    "Effect": "Allow",
                    "Principal": { "Service": "cloudtrail.amazonaws.com" },
                    "Action": "s3:PutObject",
                    // The documented grant names the area a trail delivers into, which for a
                    // trail carrying a key prefix is that prefix ahead of the layout — one
                    // resource per area, because this bucket holds both trails of the suite.
                    "Resource": [
                        format!("arn:aws:s3:::{BUCKET}/{LOG_FILE_ROOT_SEGMENT}/{ACCOUNT}/*"),
                        format!(
                            "arn:aws:s3:::{BUCKET}/{DATA_PREFIX}/{LOG_FILE_ROOT_SEGMENT}/\
                             {ACCOUNT}/*"
                        ),
                    ],
                    "Condition": {
                        "StringEquals": { "s3:x-amz-acl": "bucket-owner-full-control" },
                    },
                },
            ],
        })
        .to_string();
        let (status, text) = self.s3(
            "PUT",
            &format!("/{BUCKET}?policy"),
            &[("content-type", "application/json")],
            policy.into_bytes(),
        );
        assert!(
            (200..300).contains(&status),
            "PutBucketPolicy({BUCKET:?}) must succeed, got {status} with {text}"
        );
    }

    /// Every object key the bucket holds under one delivery area, as Amazon S3's own list
    /// reports them.
    fn delivered_keys(&self, area: &str) -> Vec<String> {
        let (status, text) = self.s3(
            "GET",
            &format!("/{BUCKET}?list-type=2&prefix={area}"),
            &[],
            Vec::new(),
        );
        assert_eq!(
            status, 200,
            "ListObjectsV2({BUCKET:?}, {area:?}) must succeed, got {status} with {text}"
        );
        let mut keys = elements(&text, "Key");
        keys.sort();
        keys
    }

    /// Look under one delivery area until it holds at least `wanted` objects, or
    /// [`SETTLE_DEADLINE`] has passed — the CONDITION is what is waited for, and the deadline
    /// only bounds a wait that will never be satisfied.
    fn delivered(&self, area: &str, wanted: usize) -> Vec<String> {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let keys = self.delivered_keys(area);
            if keys.len() >= wanted || Instant::now() >= deadline {
                return keys;
            }
            std::thread::sleep(LOOK_INTERVAL);
        }
    }

    /// Look under one delivery area [`ABSENCE_LOOKS`] times over and require it EMPTY every
    /// time, so an absence is read after the delivery latency the positive legs demonstrate
    /// rather than before it.
    fn stays_empty(&self, case: &str, area: &str) {
        for look in 1..=ABSENCE_LOOKS {
            let keys = self.delivered_keys(area);
            assert!(
                keys.is_empty(),
                "case {case:?}: expected nothing delivered under {area:?} (look {look} of \
                 {ABSENCE_LOOKS}), got {keys:?}; the log:\n{}",
                self.started.log()
            );
            std::thread::sleep(LOOK_INTERVAL);
        }
    }

    /// A user pool with one password client and one confirmed user, and the ID token that
    /// user's sign-in answers: the provider name an identity pool trusts, that client's id,
    /// and the token a login presents under the provider.
    fn user_pool_login(&self) -> (String, String, String) {
        let created = self.user_pools("CreateUserPool", &json!({ "PoolName": "delivery-pool" }));
        let pool_id = created["UserPool"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateUserPool must report an Id, got {created}"))
            .to_string();
        let client = self.user_pools(
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": pool_id,
                "ClientName": "delivery-client",
                "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
            }),
        );
        let client_id = client["UserPoolClient"]["ClientId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateUserPoolClient must report a ClientId, got {client}"))
            .to_string();
        self.user_pools(
            "AdminCreateUser",
            &json!({
                "UserPoolId": pool_id,
                "Username": POOL_USER,
                "TemporaryPassword": "Temporary-Passw0rd!1",
                "MessageAction": "SUPPRESS",
            }),
        );
        self.user_pools(
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": pool_id,
                "Username": POOL_USER,
                "Password": POOL_PASSWORD,
                "Permanent": true,
            }),
        );
        let signed_in = self.user_pools(
            "InitiateAuth",
            &json!({
                "ClientId": client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": POOL_USER, "PASSWORD": POOL_PASSWORD },
            }),
        );
        let id_token = signed_in["AuthenticationResult"]["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("InitiateAuth must report an IdToken, got {signed_in}"))
            .to_string();
        // "Amazon Cognito user pool: `cognito-idp.<region>.amazonaws.com/<YOUR_USER_POOL_ID>`"
        // — the provider name Cognito Identity's model publishes for a user pool.
        (
            format!("cognito-idp.{REGION}.amazonaws.com/{pool_id}"),
            client_id,
            id_token,
        )
    }

    /// One identity pool that admits no unauthenticated identity and trusts `provider`'s
    /// `client_id`, answering the pool's id.
    fn identity_pool(&self, name: &str, provider: &str, client_id: &str) -> String {
        let created = self.identity(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": false,
                "CognitoIdentityProviders": [
                    { "ProviderName": provider, "ClientId": client_id },
                ],
            }),
        );
        created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIdentityPool must report an id, got {created}"))
            .to_string()
    }

    /// The two calls of the identity flow against one pool, made with the login the user pool
    /// issued: mint the identity, then mint its OpenID token.
    fn identity_flow(&self, identity_pool_id: &str, provider: &str, id_token: &str) {
        let minted = self.identity(
            "GetId",
            &json!({
                "IdentityPoolId": identity_pool_id,
                "Logins": { provider: id_token },
            }),
        );
        let identity_id = minted["IdentityId"]
            .as_str()
            .unwrap_or_else(|| panic!("GetId must report an IdentityId, got {minted}"))
            .to_string();
        self.identity(
            "GetOpenIdToken",
            &json!({
                "IdentityId": identity_id,
                "Logins": { provider: id_token },
            }),
        );
    }

    /// Create one trail of this Region, delivering under `key_prefix` when one is named,
    /// answering its ARN. It is not logging yet.
    fn create_trail(&self, name: &str, key_prefix: Option<&str>) -> String {
        let mut request = json!({
            "Name": name,
            "S3BucketName": BUCKET,
            "IsMultiRegionTrail": false,
            "IncludeGlobalServiceEvents": false,
        });
        if let Some(key_prefix) = key_prefix {
            request["S3KeyPrefix"] = json!(key_prefix);
        }
        let created = self.cloudtrail("CreateTrail", &request);
        let trail_arn = created["TrailARN"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateTrail must report a TrailARN, got {created}"))
            .to_string();
        // The trail is homed where it was created, which is what its own ARN names
        // (`CreateTrailResponse` carries no `HomeRegion`:
        // `smithy_query(query="operation",operation="CreateTrail")`).
        assert_eq!(
            trail_arn,
            format!("arn:aws:cloudtrail:{REGION}:{ACCOUNT}:trail/{name}"),
            "trail {name:?} must be homed where it was created, got {created}"
        );
        trail_arn
    }

    /// Start `trail_arn` logging, and read back that it is.
    fn start_logging(&self, trail_arn: &str) {
        self.cloudtrail("StartLogging", &json!({ "Name": trail_arn }));
        let status = self.cloudtrail("GetTrailStatus", &json!({ "Name": trail_arn }));
        assert_eq!(
            status["IsLogging"],
            json!(true),
            "trail {trail_arn:?} must report it is logging, got {status}; the log:\n{}",
            self.started.log()
        );
    }
}

/// The delivery area of a trail carrying `key_prefix`, for this Region's calls.
fn delivery_area(key_prefix: Option<&str>) -> String {
    let prefix = key_prefix
        .map(|prefix| format!("{prefix}/"))
        .unwrap_or_default();
    format!("{prefix}{LOG_FILE_ROOT_SEGMENT}/{ACCOUNT}/{LOG_FILE_SERVICE_SEGMENT}/{REGION}/")
}

/// Assert `key` is a log-file key of the trail delivering under `key_prefix`: the documented
/// layout down to the DAY, and a file name carrying the account, the service and the Region.
fn assert_log_file_key(case: &str, key_prefix: Option<&str>, key: &str) {
    let area = delivery_area(key_prefix);
    let Some(rest) = key.strip_prefix(&area) else {
        panic!("case {case:?}: expected a key under {area:?}, got {key:?}");
    };
    let (day, name) = rest
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: expected a day folder in {rest:?}"));
    let parts: Vec<&str> = day.split('/').collect();
    let dated = parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts
            .iter()
            .all(|part| part.chars().all(|digit| digit.is_ascii_digit()));
    assert!(
        dated,
        "case {case:?}: expected the YYYY/MM/DD of the records, got {day:?} in {key:?}"
    );
    let expected_name = format!("{ACCOUNT}_{LOG_FILE_SERVICE_SEGMENT}_{REGION}_");
    assert!(
        name.starts_with(&expected_name) && name.ends_with(LOG_FILE_SUFFIX),
        "case {case:?}: expected a file named {expected_name}<minute>_<unique>{LOG_FILE_SUFFIX}, \
         got {name:?}"
    );
}

/// One advanced event selector admitting the data events of ONE identity pool: the category,
/// the type of the resource the call acted on, and that resource's own ARN.
fn one_pools_data_selector(pool_arn: &str) -> Value {
    json!([{
        "Name": "one-identity-pool",
        "FieldSelectors": [
            { "Field": "eventCategory", "Equals": [DATA_CATEGORY] },
            { "Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE] },
            { "Field": "resources.ARN", "Equals": [pool_arn] },
        ],
    }])
}

#[test]
fn a_trail_outside_the_leader_region_delivers_its_own_regions_calls() {
    let started = Started::start();
    let account = Account { started: &started };
    account.log_bucket();

    // The pools of the account, each trusting the user pool that signs its users in: one the
    // data selector will name, one it will not.
    let (provider, client_id, id_token) = account.user_pool_login();
    let selected_pool = account.identity_pool("delivery-selected", &provider, &client_id);
    let withheld_pool = account.identity_pool("delivery-withheld", &provider, &client_id);
    let selected_arn =
        format!("arn:aws:cognito-identity:{REGION}:{ACCOUNT}:identitypool/{selected_pool}");

    // The MANAGEMENT trail: no key prefix at all, and the selectors a create gives it — the
    // documented default a client reads back before it asks for anything else.
    let management_trail = account.create_trail(MANAGEMENT_TRAIL, None);
    let default_selectors =
        account.cloudtrail("GetEventSelectors", &json!({ "TrailName": management_trail }));
    assert_eq!(
        default_selectors["EventSelectors"][0]["DataResources"],
        json!([]),
        "case \"a trail created with default selectors\": expected no data resource, got \
         {default_selectors}"
    );
    assert_eq!(
        default_selectors.get("AdvancedEventSelectors"),
        None,
        "case \"a trail created with default selectors\": expected no AdvancedEventSelectors \
         member, got {default_selectors}"
    );

    // The DATA trail: one advanced event selector naming the `Data` category throughout, so no
    // management event can land under its prefix, narrowed to ONE pool's ARN.
    let data_trail = account.create_trail(DATA_TRAIL, Some(DATA_PREFIX));
    let naming = one_pools_data_selector(&selected_arn);
    let put = account.cloudtrail(
        "PutEventSelectors",
        &json!({ "TrailName": data_trail, "AdvancedEventSelectors": naming }),
    );
    assert_eq!(
        put["AdvancedEventSelectors"], naming,
        "case \"a selector naming one pool's ARN\": PutEventSelectors must echo it, got {put}"
    );
    let read = account.cloudtrail("GetEventSelectors", &json!({ "TrailName": data_trail }));
    assert_eq!(
        read["AdvancedEventSelectors"], naming,
        "case \"a selector naming one pool's ARN\": GetEventSelectors must report it verbatim, \
         got {read}"
    );
    assert_eq!(
        read.get("EventSelectors"),
        None,
        "case \"a selector naming one pool's ARN\": GetEventSelectors must no longer report \
         EventSelectors, got {read}"
    );
    account.start_logging(&data_trail);
    account.start_logging(&management_trail);

    // A MANAGEMENT call of this Region, which the management trail logs from the moment it
    // started: the positive control a consumer of the bucket reads before judging any absence.
    account.identity(
        "TagResource",
        &json!({
            "ResourceArn": &selected_arn,
            "Tags": { "delivery": "management-marker" },
        }),
    );
    let management_area = delivery_area(None);
    let management_keys = account.delivered(&management_area, 1);
    assert!(
        !management_keys.is_empty(),
        "case \"a management call of the Region a trail is homed in\": expected a delivered log \
         file under {management_area:?}, got none; the log:\n{}",
        started.log()
    );
    for key in &management_keys {
        assert_log_file_key(
            "a management call of the Region a trail is homed in",
            None,
            key,
        );
    }
    let status = account.cloudtrail("GetTrailStatus", &json!({ "Name": management_trail }));
    assert!(
        status["LatestDeliveryTime"].is_number(),
        "case \"a trail that has delivered a log file\": expected a LatestDeliveryTime, got \
         {status}"
    );
    assert_eq!(
        status.get("LatestDeliveryError"),
        None,
        "case \"a trail that has delivered a log file\": expected no delivery error, got {status}"
    );

    // The WITHHELD leg: the identity flow of the pool the only data-event selector of the
    // account does not name. Nothing may reach the data trail's prefix, which only a data
    // event can reach at all.
    account.identity_flow(&withheld_pool, &provider, &id_token);
    let data_area = delivery_area(Some(DATA_PREFIX));
    account.stays_empty(
        "the identity flow of a pool no selector of the account names",
        &data_area,
    );

    // The DELIVERED leg: the identical two calls of the identity flow, against the pool the
    // selector names.
    account.identity_flow(&selected_pool, &provider, &id_token);
    let delivered = account.delivered(&data_area, 1);
    assert!(
        !delivered.is_empty(),
        "case \"the identity flow of the pool the selector names\": expected the two calls \
         delivered under {data_area:?}, got nothing; the log:\n{}",
        started.log()
    );
    // At most one file per call: CloudTrail may carry both records in one file, and nothing
    // else can arrive here — a record of the withheld pair would show up as a third key.
    assert!(
        delivered.len() <= 2,
        "case \"the identity flow of the pool the selector names\": expected at most one file \
         per call under {data_area:?}, got {delivered:?}; the log:\n{}",
        started.log()
    );
    for key in &delivered {
        assert_log_file_key(
            "the identity flow of the pool the selector names",
            Some(DATA_PREFIX),
            key,
        );
    }

    // Event history of this Region reports the management marker and no data event of either
    // pool — a data event is observable in a delivered log file alone.
    let looked_up = account.cloudtrail("LookupEvents", &json!({ "MaxResults": 50 }));
    let history: Vec<String> = looked_up["Events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["EventName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        history.iter().any(|name| name == "TagResource"),
        "case \"event history of the Region the calls were served in\": expected the management \
         marker, got {history:?}"
    );
    for withheld_name in ["GetId", "GetOpenIdToken"] {
        assert!(
            !history.iter().any(|name| name == withheld_name),
            "case \"event history of the Region the calls were served in\": expected no data \
             event, got {withheld_name} in {history:?}"
        );
    }
}
