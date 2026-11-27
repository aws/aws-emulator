// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FSx for NetApp ONTAP cluster-peer slice, driven over the wire.
//!
//! RULE: a cluster peer is a resource of its own, created ON an ONTAP file system this
//! account has: an accepted create persists the peer in `CREATING` and answers with the
//! stored record, the clock then owes it `CREATED`, `DescribeClusterPeers` reports exactly
//! those stored values, `UpdateClusterPeer` replaces the members it carries and leaves the
//! rest, and `DeleteClusterPeer` moves the record to `DELETING` and lets the clock remove
//! it. A file system the account does not have, one of any other type, and
//! `AllowedSvmIds` outside the one mode that admits them are each refused with the error the
//! model declares for that class, and nothing is stored.
//!
//! Every case goes through the same front door a client uses, through the helpers in the
//! parent suite: nothing here reaches into the store directly, so a response that is not
//! computed from persisted state cannot pass. The tables span what the rule generalises over
//! — the preauthorization mode, the number and family of the peer addresses, which optional
//! members the request carries, and the file-system type — rather than one recorded request.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, emulator, ok, with_client_request_token, Emulator, ACCOUNT, REGION,
};
use crate::not_found_messages::error_of;

/// How long a wait below gives a clock-owed transition to be materialised: far longer than
/// the service's own transition span, so what the wait is bounded by is the CONDITION and
/// not how fast the host runs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
/// How long a wait below pauses between two looks.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// The modelled `ClusterPeerLifecycle` values this suite observes
/// (`smithy_query(query="shape", shape="ClusterPeerLifecycle")`: CREATING | CREATED |
/// DELETING | FAILED).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const CREATED: &str = "CREATED";
/// See [`CREATING`].
const DELETING: &str = "DELETING";

/// A Secrets Manager secret ARN satisfying the modelled `CustomerSecretsManagerARN`
/// (`length 64..1024`, `^arn:[^:]{1,63}:secretsmanager:[a-z0-9-]+:[0-9]{12}:secret:[a-zA-Z0-9/_+=.@-]+-[a-zA-Z0-9]{6}$`).
/// The secret belongs to Secrets Manager, and this operation models no error for one that
/// cannot be read, so the value only has to be a well-formed ARN.
const PASSPHRASE_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:fsx-cluster-peering-passphrase-AbCdEf";

/// An SVM id satisfying the modelled `StorageVirtualMachineId` (`length 21..21`,
/// `^(svm-[0-9a-f]{17,})$`), for the `SPECIFIED` preauthorization list.
const PREAUTHORIZED_SVM: &str = "svm-0123456789abcdef0";
/// A second one, so a case can preauthorize more than one SVM.
const OTHER_PREAUTHORIZED_SVM: &str = "svm-00000000000000001";

/// Create an ONTAP file system and answer its `FileSystemId`.
///
/// Cluster peering is an ONTAP capability, so every accepted case below is created on one of
/// these. The file system is minted through the service's own front door, so the id a peer is
/// created on is one this emulator really holds.
async fn ontap_file_system(router: &Emulator) -> String {
    let created = ok(
        router,
        "CreateFileSystem",
        &with_client_request_token(&json!({
            "FileSystemType": "ONTAP",
            "StorageCapacity": 1200,
            "StorageType": "SSD",
            "SubnetIds": [router.subnet_a(), router.subnet_b()],
        })),
    )
    .await;
    created["FileSystem"]["FileSystemId"]
        .as_str()
        .expect("CreateFileSystem must answer a FileSystemId")
        .to_string()
}

/// A `CreateClusterPeer` request body, carrying the members every case needs.
fn create_request(file_system_id: &str, addresses: &[&str], allowed_svm_mode: &str) -> Value {
    with_client_request_token(&json!({
        "FileSystemId": file_system_id,
        "PeerAddresses": addresses,
        "PassphraseSecretArn": PASSPHRASE_SECRET_ARN,
        "AllowedSvmMode": allowed_svm_mode,
    }))
}

/// Every cluster peer this store holds, as `DescribeClusterPeers` reports them.
async fn described_peers(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "DescribeClusterPeers", &json!({}))
        .await
        .get("ClusterPeers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The stored cluster peer with this id, as the unfiltered listing reports it, or `None`
/// when the store holds none.
async fn described_peer(router: &MultiServiceRouter, cluster_peer_id: &str) -> Option<Value> {
    described_peers(router)
        .await
        .into_iter()
        .find(|peer| peer["ClusterPeerId"] == json!(cluster_peer_id))
}

/// Look again until the peer's `Lifecycle` is `expected`, or [`SETTLE_DEADLINE`] passes.
///
/// A transition this service owes the clock is never slept for: the condition is polled, so
/// the case is bounded by what the store reports and not by how fast the host runs.
async fn await_lifecycle(router: &MultiServiceRouter, cluster_peer_id: &str, expected: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let reported = described_peer(router, cluster_peer_id)
            .await
            .map(|peer| peer["Lifecycle"].clone());
        if reported == Some(json!(expected)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "peer {cluster_peer_id}: expected Lifecycle {expected}, got {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Look again until the store holds no peer with this id, or [`SETTLE_DEADLINE`] passes.
async fn await_removed(router: &MultiServiceRouter, cluster_peer_id: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let reported = described_peer(router, cluster_peer_id).await;
        if reported.is_none() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "peer {cluster_peer_id}: expected the clock to remove the record, still got \
             {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Whether `id` matches the modelled `ClusterPeerId` pattern
/// `^(clusterpeer-[0-9a-f]{17})$`.
fn is_well_formed_cluster_peer_id(id: &str) -> bool {
    id.strip_prefix("clusterpeer-").is_some_and(|hex| {
        hex.len() == 17 && hex.chars().all(|digit| digit.is_ascii_hexdigit() && !digit.is_uppercase())
    })
}

/// Whether `uuid` is a hyphenated UUID, as the modelled `UUID` (`length 0..36`) carries it:
/// lowercase hex in `8-4-4-4-12` groups.
fn is_hyphenated_uuid(uuid: &str) -> bool {
    let groups: Vec<&str> = uuid.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit()))
}

/// One row of the create table: the preauthorization mode, the intercluster peer addresses,
/// the preauthorized SVM ids, and the optional peer-cluster name.
type CreateCase<'row> = (
    &'row str,
    &'row [&'row str],
    &'row [&'row str],
    Option<&'row str>,
);

/// RULE: a created cluster peer is persisted against the ONTAP file system it was created
/// on, the response is that stored record, and `DescribeClusterPeers` reports the very same
/// values — whatever preauthorization mode, peer addresses and optional members the request
/// carried.
#[tokio::test]
async fn create_persists_a_cluster_peer_and_describe_reports_it_back() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;

    // (AllowedSvmMode, PeerAddresses, AllowedSvmIds, PeerClusterName): the modelled
    // `AllowedSvmMode` enum (ALL | NONE | SPECIFIED), one and two intercluster addresses of
    // both IP families, and the optional members present and absent.
    let cases: &[CreateCase<'_>] = &[
        ("ALL", &["10.0.1.10"], &[], Some("PeerClusterOne")),
        ("NONE", &["10.0.1.11", "10.0.2.11"], &[], None),
        (
            "SPECIFIED",
            &["2001:db8::1"],
            &[PREAUTHORIZED_SVM],
            Some("PeerClusterThree"),
        ),
        (
            "SPECIFIED",
            &["10.0.1.13", "2001:db8::13"],
            &[PREAUTHORIZED_SVM, OTHER_PREAUTHORIZED_SVM],
            None,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    let mut uuids: Vec<String> = Vec::new();
    for case in cases {
        let (allowed_svm_mode, addresses, allowed_svm_ids, peer_cluster_name) = *case;
        let mut body = create_request(&file_system_id, addresses, allowed_svm_mode);
        if !allowed_svm_ids.is_empty() {
            body["AllowedSvmIds"] = json!(allowed_svm_ids);
        }
        if let Some(peer_cluster_name) = peer_cluster_name {
            body["PeerClusterName"] = json!(peer_cluster_name);
        }

        let peer = ok(&router, "CreateClusterPeer", &body).await["ClusterPeer"].clone();
        let id = peer["ClusterPeerId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_well_formed_cluster_peer_id(&id),
            "case {case:?}: expected a ClusterPeerId matching ^clusterpeer-[0-9a-f]{{17}}$, got \
             {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh ClusterPeerId, got the already-minted {id:?}"
        );
        assert_eq!(
            peer["Lifecycle"],
            json!(CREATING),
            "case {case:?}: expected Lifecycle {CREATING} on create, got {:?}",
            peer["Lifecycle"]
        );
        assert_eq!(
            peer["FileSystemId"],
            json!(file_system_id),
            "case {case:?}: expected the peer to be created on {file_system_id}, got {:?}",
            peer["FileSystemId"]
        );
        assert_eq!(
            peer["ResourceARN"],
            json!(format!("arn:aws:fsx:{REGION}:{ACCOUNT}:cluster-peer/{id}")),
            "case {case:?}: expected the cluster-peer ARN for the signed region, got {:?}",
            peer["ResourceARN"]
        );
        assert_eq!(
            peer["PeerAddresses"],
            json!(addresses),
            "case {case:?}: expected the stated PeerAddresses, got {:?}",
            peer["PeerAddresses"]
        );
        assert_eq!(
            peer["PassphraseSecretArn"],
            json!(PASSPHRASE_SECRET_ARN),
            "case {case:?}: expected the stated PassphraseSecretArn, got {:?}",
            peer["PassphraseSecretArn"]
        );
        assert_eq!(
            peer["AllowedSvmMode"],
            json!(allowed_svm_mode),
            "case {case:?}: expected the stated AllowedSvmMode, got {:?}",
            peer["AllowedSvmMode"]
        );
        assert_eq!(
            peer["PeerClusterName"],
            peer_cluster_name.map_or(Value::Null, |name| json!(name)),
            "case {case:?}: expected the stated PeerClusterName and nothing where the request \
             named none, got {:?}",
            peer["PeerClusterName"]
        );
        if allowed_svm_ids.is_empty() {
            assert_eq!(
                peer["AllowedSvmIds"],
                Value::Null,
                "case {case:?}: a peer preauthorizing no named SVM has no AllowedSvmIds to \
                 report, got {:?}",
                peer["AllowedSvmIds"]
            );
        } else {
            assert_eq!(
                peer["AllowedSvmIds"],
                json!(allowed_svm_ids),
                "case {case:?}: expected the stated AllowedSvmIds, got {:?}",
                peer["AllowedSvmIds"]
            );
        }
        let uuid = peer["UUID"].as_str().unwrap_or_default().to_string();
        assert!(
            is_hyphenated_uuid(&uuid),
            "case {case:?}: expected a hyphenated UUID, got {uuid:?}"
        );
        assert!(
            !uuids.contains(&uuid),
            "case {case:?}: expected a UUID of this peer alone, got the already-reported \
             {uuid:?}"
        );

        // The persisted record, read back through the implemented reader.
        let described = described_peer(&router, &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeClusterPeers must report {id}"));
        for member in [
            "ClusterPeerId",
            "FileSystemId",
            "ResourceARN",
            "PeerAddresses",
            "PeerClusterName",
            "PassphraseSecretArn",
            "AllowedSvmMode",
            "AllowedSvmIds",
            "UUID",
        ] {
            assert_eq!(
                described[member], peer[member],
                "case {case:?}: DescribeClusterPeers must report the stored {member}; create \
                 said {:?} and describe said {:?}",
                peer[member], described[member]
            );
        }

        minted.push(id);
        uuids.push(uuid);
    }

    // The clock owes each created peer its way to `CREATED`, and the id stays the same
    // record throughout.
    for id in &minted {
        await_lifecycle(&router, id, CREATED).await;
    }
}

/// RULE: the tags a create states live on the created peer, so the tagging operations read
/// and write the same stored map through the peer's own `ResourceARN`.
#[tokio::test]
async fn cluster_peer_tags_are_stored_once_and_read_back_everywhere() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;

    let mut body = create_request(&file_system_id, &["10.0.3.10"], "NONE");
    body["Tags"] = json!([
        { "Key": "environment", "Value": "staging" },
        { "Key": "owner", "Value": "storage" },
    ]);
    let peer = ok(&router, "CreateClusterPeer", &body).await["ClusterPeer"].clone();
    let resource_arn = peer["ResourceARN"].as_str().unwrap_or_default().to_string();

    let listed = ok(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceARN": resource_arn }),
    )
    .await;
    let mut reported: Vec<(String, String)> = listed["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        vec![
            ("environment".to_string(), "staging".to_string()),
            ("owner".to_string(), "storage".to_string()),
        ],
        "ListTagsForResource must report the tags the create stored on the peer, got \
         {reported:?}"
    );

    // A tag written through the tagging operation reaches the same stored map the peer
    // reports.
    ok(
        &router,
        "TagResource",
        &json!({
            "ResourceARN": resource_arn,
            "Tags": [{ "Key": "team", "Value": "ontap" }],
        }),
    )
    .await;
    let described = described_peer(&router, peer["ClusterPeerId"].as_str().unwrap_or_default())
        .await
        .expect("DescribeClusterPeers must report the peer");
    let keys: Vec<String> = described["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        keys.contains(&"team".to_string()),
        "the peer must report the tag TagResource wrote, got {keys:?}"
    );
}

/// RULE: a create that names a file system the account does not have, one whose type cannot
/// hold a cluster peer, or `AllowedSvmIds` outside the one mode that admits them is refused
/// with the error the model declares for that class — and stores nothing.
#[tokio::test]
async fn create_is_refused_where_the_request_cannot_be_served() {
    let router = emulator().await;

    // Every other member of the modelled `FileSystemType` enum, each with a `StorageType`
    // it supports: cluster peering is an ONTAP capability, so each of these is
    // `UnsupportedOperation`.
    for (file_system_type, storage_type) in [
        ("WINDOWS", "SSD"),
        ("LUSTRE", "SSD"),
        ("OPENZFS", "INTELLIGENT_TIERING"),
    ] {
        let created = ok(
            &router,
            "CreateFileSystem",
            &with_client_request_token(&json!({
                "FileSystemType": file_system_type,
                "StorageCapacity": 1200,
                "StorageType": storage_type,
                "SubnetIds": [router.subnet_a()],
            })),
        )
        .await;
        let id = created["FileSystem"]["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, code, message) = error_of(
            &router,
            "CreateClusterPeer",
            &create_request(&id, &["10.0.4.10"], "ALL"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "UnsupportedOperation"),
            "case {file_system_type}: expected HTTP 400 UnsupportedOperation, got {status} \
             {code}"
        );
        assert_eq!(
            message,
            format!("Cluster peering is not supported for file systems of type {file_system_type}."),
            "case {file_system_type}: expected the message to name the file system's type, got \
             {message:?}"
        );
    }

    // A `FileSystemId` the account has no file system for: the modelled `FileSystemNotFound`,
    // naming the id the request carried.
    let absent = "fs-0000000000badbeef";
    let (status, code, message) = error_of(
        &router,
        "CreateClusterPeer",
        &create_request(absent, &["10.0.4.11"], "ALL"),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "expected HTTP 400 FileSystemNotFound for an absent file system, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("File system '{absent}' does not exist."),
        "expected the message to name the requested file system, got {message:?}"
    );

    // "You can specify AllowedSvmIds only when AllowedSvmMode is SPECIFIED": both other
    // modes refuse the list.
    let file_system_id = ontap_file_system(&router).await;
    for allowed_svm_mode in ["ALL", "NONE"] {
        let mut body = create_request(&file_system_id, &["10.0.4.12"], allowed_svm_mode);
        body["AllowedSvmIds"] = json!([PREAUTHORIZED_SVM]);
        let (status, code, message) = error_of(&router, "CreateClusterPeer", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {allowed_svm_mode}: expected HTTP 400 BadRequest for AllowedSvmIds outside \
             SPECIFIED, got {status} {code}"
        );
        assert_eq!(
            message, "You can specify AllowedSvmIds only when AllowedSvmMode is SPECIFIED.",
            "case {allowed_svm_mode}: expected the documented requirement as the message, got \
             {message:?}"
        );
    }

    // Nothing above stored a peer.
    let held = described_peers(&router).await;
    assert!(
        held.is_empty(),
        "a refused create must store nothing, got {held:?}"
    );
}

/// RULE: `DescribeClusterPeers` selects by `ClusterPeerIds` and by `Filters` TOGETHER, pages
/// at `MaxResults` with an opaque `NextToken`, and refuses an id it holds nothing for — or
/// one a filter excludes — with `ClusterPeerNotFound`.
#[tokio::test]
async fn describe_selects_by_id_and_filter_together_and_pages() {
    let router = emulator().await;
    let first_file_system = ontap_file_system(&router).await;
    let second_file_system = ontap_file_system(&router).await;

    let mut on_first: Vec<String> = Vec::new();
    for address in ["10.0.5.10", "10.0.5.11", "10.0.5.12"] {
        let peer = ok(
            &router,
            "CreateClusterPeer",
            &create_request(&first_file_system, &[address], "ALL"),
        )
        .await["ClusterPeer"]
            .clone();
        on_first.push(peer["ClusterPeerId"].as_str().unwrap_or_default().to_string());
    }
    let on_second = ok(
        &router,
        "CreateClusterPeer",
        &create_request(&second_file_system, &["10.0.6.10"], "ALL"),
    )
    .await["ClusterPeer"]["ClusterPeerId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The `file-system-id` filter, which is the one name this listing selects by.
    let filtered = ok(
        &router,
        "DescribeClusterPeers",
        &json!({
            "Filters": [{ "Name": "file-system-id", "Values": [first_file_system] }],
        }),
    )
    .await;
    let mut selected: Vec<String> = filtered["ClusterPeers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|peer| peer["ClusterPeerId"].as_str().unwrap_or_default().to_string())
        .collect();
    selected.sort();
    let mut expected = on_first.clone();
    expected.sort();
    assert_eq!(
        selected, expected,
        "the file-system-id filter must select exactly the peers of that file system, got \
         {selected:?}"
    );

    // One page at a time: every record is walked exactly once, and the token is opaque.
    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 1 });
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(&router, "DescribeClusterPeers", &body).await;
        let ids: Vec<String> = page["ClusterPeers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|peer| peer["ClusterPeerId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            ids.len() <= 1,
            "a page of MaxResults 1 must hold at most one peer, got {ids:?}"
        );
        walked.extend(ids);
        next_token = page["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            walked.len() <= 4,
            "paging must end: walked {walked:?} over four stored peers"
        );
    }
    walked.sort();
    let mut every_peer = on_first.clone();
    every_peer.push(on_second.clone());
    every_peer.sort();
    assert_eq!(
        walked, every_peer,
        "paging must walk every stored peer exactly once, got {walked:?}"
    );

    // An id this store holds nothing for: `ClusterPeerNotFound`, naming it.
    let absent = "clusterpeer-0000000000badbeef";
    let (status, code, message) = error_of(
        &router,
        "DescribeClusterPeers",
        &json!({ "ClusterPeerIds": [absent] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ClusterPeerNotFound"),
        "expected HTTP 400 ClusterPeerNotFound for an absent id, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("Cluster peer '{absent}' does not exist."),
        "expected the message to name the requested id, got {message:?}"
    );

    // An id the filters EXCLUDE is refused the same way: the ids and the filters select
    // together.
    let (status, code, _) = error_of(
        &router,
        "DescribeClusterPeers",
        &json!({
            "ClusterPeerIds": [on_second],
            "Filters": [{ "Name": "file-system-id", "Values": [first_file_system] }],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ClusterPeerNotFound"),
        "expected HTTP 400 ClusterPeerNotFound for an id the filter excludes, got {status} \
         {code}"
    );

    // A `Filter` this listing cannot read as a selection is a bad request, not an empty
    // page: a name outside the modelled `ClusterPeerFilterName` enum is refused by the
    // modelled constraint before the handler runs, and a filter carrying no name at all is
    // refused by the handler, naming the names this listing accepts.
    let (status, code, _) = error_of(
        &router,
        "DescribeClusterPeers",
        &json!({ "Filters": [{ "Name": "storage-virtual-machine-id", "Values": ["svm-0"] }] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for a filter name outside the modelled enum, got \
         {status} {code}"
    );
    let (status, code, message) = error_of(
        &router,
        "DescribeClusterPeers",
        &json!({ "Filters": [{ "Values": [first_file_system] }] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for a filter that names nothing, got {status} {code}"
    );
    assert!(
        message.contains("file-system-id"),
        "expected the message to name the supported filter names, got {message:?}"
    );

    // A token this service did not mint means nothing to it.
    let (status, code, _) = error_of(
        &router,
        "DescribeClusterPeers",
        &json!({ "NextToken": "bm90LWEtdG9rZW4=" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for a token this service did not mint, got {status} \
         {code}"
    );
}

/// RULE: every member an update carries replaces the stored one and is what later reads
/// report; an absent member leaves the stored value alone.
#[tokio::test]
async fn update_mutates_the_stored_record_member_by_member() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;
    let mut body = create_request(&file_system_id, &["10.0.7.10"], "ALL");
    body["PeerClusterName"] = json!("OriginalPeerCluster");
    let created = ok(&router, "CreateClusterPeer", &body).await["ClusterPeer"].clone();
    let id = created["ClusterPeerId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (the update body, the members it must leave alone)
    let cases: &[(Value, &[&str])] = &[
        (
            json!({ "PeerClusterName": "RenamedPeerCluster" }),
            &["PeerAddresses"],
        ),
        (
            json!({ "PeerAddresses": ["10.0.7.20", "2001:db8::20"] }),
            &["PeerClusterName"],
        ),
        (
            json!({
                "PeerClusterName": "BothAtOnce",
                "PeerAddresses": ["10.0.7.30"],
            }),
            &[],
        ),
    ];

    for case in cases {
        let (update, untouched) = case;
        let before = described_peer(&router, &id)
            .await
            .expect("the peer must be stored");
        let mut request = update.clone();
        request["ClusterPeerId"] = json!(id);
        let answered = ok(&router, "UpdateClusterPeer", &request).await["ClusterPeer"].clone();

        for (member, value) in update.as_object().expect("the update is an object") {
            assert_eq!(
                answered[member], *value,
                "case {case:?}: expected the update to replace {member}, got {:?}",
                answered[member]
            );
        }
        for member in *untouched {
            assert_eq!(
                answered[*member], before[*member],
                "case {case:?}: expected {member} to be left as it was ({:?}), got {:?}",
                before[*member], answered[*member]
            );
        }
        // The answer is the stored record.
        let described = described_peer(&router, &id)
            .await
            .expect("the peer must still be stored");
        assert_eq!(
            described, answered,
            "case {case:?}: DescribeClusterPeers must report what the update answered"
        );
    }

    // An id this store holds nothing for is refused, and nothing is changed.
    let absent = "clusterpeer-0000000000badbeef";
    let (status, code, message) = error_of(
        &router,
        "UpdateClusterPeer",
        &json!({ "ClusterPeerId": absent, "PeerClusterName": "Nowhere" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ClusterPeerNotFound"),
        "expected HTTP 400 ClusterPeerNotFound for an absent id, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("Cluster peer '{absent}' does not exist."),
        "expected the message to name the requested id, got {message:?}"
    );
}

/// RULE: a delete moves the stored peer to `DELETING` and the clock removes it, so the
/// answer reports the lifecycle the peer now has and a later read no longer holds it.
#[tokio::test]
async fn delete_moves_the_peer_to_deleting_and_the_clock_removes_it() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;
    let id = ok(
        &router,
        "CreateClusterPeer",
        &create_request(&file_system_id, &["10.0.8.10"], "ALL"),
    )
    .await["ClusterPeer"]["ClusterPeerId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let deleted = ok(
        &router,
        "DeleteClusterPeer",
        &with_client_request_token(&json!({ "ClusterPeerId": id })),
    )
    .await;
    assert_eq!(
        deleted["ClusterPeerId"],
        json!(id),
        "the delete must answer the id it deleted, got {:?}",
        deleted["ClusterPeerId"]
    );
    assert_eq!(
        deleted["Lifecycle"],
        json!(DELETING),
        "the delete must answer the lifecycle the peer now has, got {:?}",
        deleted["Lifecycle"]
    );

    // An update of a peer on its way out has nothing left to change.
    let (status, code, _) = error_of(
        &router,
        "UpdateClusterPeer",
        &json!({ "ClusterPeerId": id, "PeerClusterName": "TooLate" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for an update of a deleting peer, got {status} {code}"
    );

    await_removed(&router, &id).await;

    // Once the record is gone, the id resolves to nothing.
    let (status, code, _) = error_of(
        &router,
        "DeleteClusterPeer",
        &with_client_request_token(&json!({ "ClusterPeerId": id })),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ClusterPeerNotFound"),
        "expected HTTP 400 ClusterPeerNotFound once the peer is removed, got {status} {code}"
    );
}

/// RULE: one `ClientRequestToken` stands for one create. Replaying it with the same
/// parameters answers with the peer it already created; replaying it with different
/// parameters is `IncompatibleParameterError`.
#[tokio::test]
async fn create_replays_one_client_request_token_to_one_cluster_peer() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;
    let mut body = create_request(&file_system_id, &["10.0.9.10"], "ALL");
    body["ClientRequestToken"] = json!("one-cluster-peer-token");

    let first = ok(&router, "CreateClusterPeer", &body).await["ClusterPeer"].clone();
    let replayed = ok(&router, "CreateClusterPeer", &body).await["ClusterPeer"].clone();
    assert_eq!(
        replayed["ClusterPeerId"], first["ClusterPeerId"],
        "a replay with the same parameters must answer with the peer the token created, got \
         {:?}",
        replayed["ClusterPeerId"]
    );
    let held = described_peers(&router).await;
    assert_eq!(
        held.len(),
        1,
        "a replayed create must not store a second peer, got {held:?}"
    );

    // Each differing member is its own case: any parameter that decides what the peer IS
    // makes the replay incompatible.
    let changes: &[(&str, Value)] = &[
        ("PeerAddresses", json!(["10.0.9.99"])),
        ("AllowedSvmMode", json!("NONE")),
        ("PeerClusterName", json!("DifferentPeerCluster")),
    ];
    for case in changes {
        let (member, value) = case;
        let mut changed = body.clone();
        changed[*member] = value.clone();
        let (status, code, _) = error_of(&router, "CreateClusterPeer", &changed).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IncompatibleParameterError"),
            "case {member}: expected HTTP 400 IncompatibleParameterError for a replay with a \
             different {member}, got {status} {code}"
        );
    }

    // The peer the token created is untouched by every refused replay.
    let (status, body) = call(
        &router,
        "DescribeClusterPeers",
        &json!({ "ClusterPeerIds": [first["ClusterPeerId"].clone()] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the peer the token created must still be stored, got HTTP {status} with {body}"
    );
}
