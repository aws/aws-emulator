// SPDX-License-Identifier: CC0-1.0

//! The CONNECTED (Payments) ACCOUNT a monetization configuration names, end to end over
//! the wire.
//!
//! The rule under test: a save that configures monetization settles its payments into
//! the connected accounts `MonetizationConfig$ConnectedAccountConfigs` names, and WAF
//! RESOLVES each of them before it writes the document — a reference that resolves to
//! nothing is the modelled `WAFNonexistentItemException` (client, HTTP 400, bound by
//! every one of the four operations that carry the configuration:
//! `smithy_query(query="operation",operation="CreateWebACL")` and its three siblings),
//! carrying WAF's fixed sentence for that condition, and nothing is stored. A connected
//! account is a Payments resource, created in that console and only ever resolved here;
//! this emulator serves no Payments control plane, so the set a save resolves against
//! holds none and every reference resolves to nothing — while a configuration that names
//! NO connected account is stored and reads back verbatim, whatever else the request
//! carries.
//!
//! The rows differ along what the rule generalises over: how the reference is addressed
//! inside the form `ConnectedAccountArn` pins (the partition, the account, and the
//! spelling of the account name), the pricing the entry carries — how many of the two
//! networks and the price amounts at both published bounds — what the configuration
//! carries beside the entry, and WHICH of the four saving operations is asked — a create
//! and an update, of a web ACL and of a rule group. The complement rows are
//! configurations that name no account at all, including requests that carry a Payments
//! ARN in a member that has nothing to do with settlement, which are created normally;
//! and the refusals that are NOT the resolution's are spanned too — a partition connected
//! accounts are not available in, and a configuration the model's own bounds do not
//! allow.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{visibility, Waf};

/// The one namespace monetization is served in: the `Monetize` action "is available only
/// for web ACLs associated with Amazon CloudFront distributions"
/// (`smithy_query(query="shape",shape="MonetizeAction")`), which is the CLOUDFRONT scope.
const MONETIZED_SCOPE: &str = "CLOUDFRONT";

/// The other namespace, which serves no monetization at all.
const REGIONAL_SCOPE: &str = "REGIONAL";

/// The wire error code (`__type`) of the modelled refusal for something the request names
/// that does not exist.
const NONEXISTENT_ITEM_CODE: &str = "WAFNonexistentItemException";

/// The wire error code of the refusal for an operation that does not exist in the
/// namespace it was asked for, which is what monetization asked for regionally earns.
const INVALID_OPERATION_CODE: &str = "WAFInvalidOperationException";

/// The sentence WAF answers a save whose connected account resolves to nothing with, as
/// the recorded WAF responses word it: fixed text in full, naming the member a caller
/// corrects rather than echoing the reference, so the same sentence answers every such
/// save.
const CONNECTED_ACCOUNT_NOT_FOUND: &str = "The specified connected account does not \
     exist. Verify the ConnectedAccountArn. If the account was deleted and recreated, it \
     has a new ARN - use the current one from the Payments console.";

/// The sentence WAF answers a monetization setting asked for in the regional namespace
/// with (`crate::refusal_classes` spans the same sentence over the monetization reads).
const MONETIZATION_NOT_AVAILABLE: &str = "Monetization is not available for regional \
     resources. Use CloudFront scope to configure monetization.";

/// A well-formed `MonetizationConfig` naming no connected account: one production payment
/// network with a wallet address and a price per request, in the matching production
/// currency mode.
fn crypto_config() -> Value {
    json!({
        "CryptoConfig": {
            "PaymentNetworks": [{
                "Chain": "BASE",
                "WalletAddress": "0x00000000000000000000000000000000000000a1",
                "Prices": [{ "Amount": "0.01", "Currency": "USDC" }],
            }]
        },
        "CurrencyMode": "REAL",
    })
}

/// A connected-account ARN of the form `ConnectedAccountArn` pins:
/// `^arn:aws(-[a-z]+)*:payments::[0-9]{12}:connectedaccount/[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}-[A-Za-z0-9]{6}$`
/// (`smithy_query(query="shape",shape="ConnectedAccountArn")`), in `partition` and for the
/// account `account`, naming the account `name`.
fn connected_account_arn(partition: &str, account: &str, name: &str) -> String {
    format!("arn:{partition}:payments::{account}:connectedaccount/{name}")
}

/// One per-network configuration of a connected account: the chain and the one price
/// entry `ConnectedAccountChainConfig$Prices` allows ("currently supports a single price
/// entry in USDC"), in the currency `CryptoCurrency` declares.
fn connected_account_network(chain: &str, amount: &str) -> Value {
    json!({
        "Chain": chain,
        "Prices": [{ "Amount": amount, "Currency": "USDC" }],
    })
}

/// One row: a `MonetizationConfig` whose `ConnectedAccountConfigs` names an account, and
/// how that reference and its pricing are written.
struct ConnectedAccountRow {
    case: &'static str,
    config: Value,
}

/// The connected-account configurations a save can carry, each written into a
/// monetization configuration that differs in what it carries beside it.
///
/// `ConnectedAccountConfigs` is `length(1..1)` — "currently supports exactly one
/// connected account" — and an entry carries both `ConnectedAccountArn` and
/// `PaymentNetworks` `@required`, the latter `length(1..2)` ("you can specify 1 to 2
/// networks"), so the rows differ along what the model leaves open: the partition and the
/// account the reference is addressed in, the spelling of the account name inside the
/// published pattern, how many networks the entry prices and which chains they are
/// (`ConnectedAccountChain` → `BASE`, `SOLANA`), the price amounts at both published
/// bounds ("valid values range from 0.01 to 999999.99 USDC"), and what the configuration
/// carries beside the entry.
fn connected_account_rows() -> Vec<ConnectedAccountRow> {
    vec![
        ConnectedAccountRow {
            case: "one account on one network, at the lowest price allowed",
            config: json!({
                "ConnectedAccountConfigs": [{
                    "ConnectedAccountArn":
                        connected_account_arn("aws", "000000000000", "acct-ABC123"),
                    "PaymentNetworks": [connected_account_network("BASE", "0.01")],
                }],
            }),
        },
        ConnectedAccountRow {
            case: "one account on both networks, at the highest price allowed",
            config: json!({
                "ConnectedAccountConfigs": [{
                    "ConnectedAccountArn":
                        connected_account_arn("aws", "111122223333", "payments_acct_01-a1B2c3"),
                    "PaymentNetworks": [
                        connected_account_network("BASE", "999999.99"),
                        connected_account_network("SOLANA", "12.50"),
                    ],
                }],
            }),
        },
        ConnectedAccountRow {
            case: "one account beside a well-formed crypto configuration",
            config: {
                let mut config = crypto_config();
                config["ConnectedAccountConfigs"] = json!([{
                    "ConnectedAccountArn":
                        connected_account_arn("aws", "210987654321", "8f1d2c3b4a5e6f70-Zz9900"),
                    // The crypto configuration above prices BASE, and "a given
                    // blockchain network (chain) can be used by only one of them"
                    // (`MonetizationConfig$ConnectedAccountConfigs`), so this prices the
                    // other network.
                    "PaymentNetworks": [connected_account_network("SOLANA", "0.25")],
                }]);
                config
            },
        },
        ConnectedAccountRow {
            case: "an account name at the long end of the published pattern",
            config: json!({
                "CurrencyMode": "REAL",
                "ConnectedAccountConfigs": [{
                    "ConnectedAccountArn": connected_account_arn(
                        "aws",
                        "000000000000",
                        "a0123456789012345678901234567890123456789012345678901234567890b-Q7q7Q7",
                    ),
                    "PaymentNetworks": [connected_account_network("SOLANA", "3")],
                }],
            }),
        },
    ]
}

/// A `MonetizationConfig` that names NO connected account, by row: the well-formed
/// configuration, the currency mode alone and a testnet network — none of them names an
/// account, so none engages the precondition.
///
/// A configuration carrying an EMPTY `ConnectedAccountConfigs`, or an entry that names no
/// account, is not among them: the model bounds the list at `length(1..1)` and marks both
/// members of an entry `@required`, so each of those is a member-constraint failure
/// ([`a_connected_account_configuration_the_model_does_not_allow_is_refused`]).
fn unreferencing_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("a well-formed crypto configuration", crypto_config()),
        ("the currency mode alone", json!({ "CurrencyMode": "TEST" })),
        (
            "a testnet network with a wallet address and its price",
            json!({
                "CryptoConfig": {
                    "PaymentNetworks": [{
                        "Chain": "SOLANA_DEVNET",
                        "WalletAddress": "So11111111111111111111111111111111111111112",
                        "Prices": [{ "Amount": "0.25", "Currency": "USDC" }],
                    }]
                },
                "CurrencyMode": "TEST",
            }),
        ),
    ]
}

/// Assert that `status`/`body` is the refusal a save naming a connected account that does
/// not exist earns: the modelled code and WAF's whole fixed sentence.
fn assert_connected_account_not_found(status: u16, body: &Value, case: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the save to be refused with 400, got status {status} and \
         body {body}"
    );
    let code = body["__type"].as_str().unwrap_or_default();
    assert!(
        code.ends_with(NONEXISTENT_ITEM_CODE),
        "case {case:?}: expected the error code {NONEXISTENT_ITEM_CODE}, got {code:?} in \
         body {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(CONNECTED_ACCOUNT_NOT_FOUND),
        "case {case:?}: expected WAF's sentence for a connected account that does not \
         exist, got {:?}",
        body["Message"]
    );
}

/// The names a namespace's listing holds, for asserting that a refused create stored
/// nothing.
async fn names_in_scope(waf: &Waf, operation: &str, member: &str, scope: &str) -> Vec<String> {
    let listed = waf.ok(operation, json!({ "Scope": scope })).await;
    listed[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["Name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A CREATE whose monetization configuration names a connected account is refused with
/// WAF's fixed sentence, and nothing is created — for a web ACL and for a rule group, over
/// every ARN form the member can carry.
#[tokio::test]
async fn a_create_naming_a_connected_account_is_refused_and_stores_nothing() {
    let waf = Waf::new();

    for (index, row) in connected_account_rows().iter().enumerate() {
        let acl_name = format!("connected-acl-{index}");
        let case = format!("CreateWebACL with {}", row.case);
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": MONETIZED_SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "MonetizationConfig": row.config.clone(),
                }),
            )
            .await;
        assert_connected_account_not_found(status, &body, &case);
        let stored = names_in_scope(&waf, "ListWebACLs", "WebACLs", MONETIZED_SCOPE).await;
        assert!(
            !stored.contains(&acl_name),
            "case {case:?}: a refused create must store no web ACL, got {stored:?}"
        );

        let group_name = format!("connected-group-{index}");
        let case = format!("CreateRuleGroup with {}", row.case);
        let (status, body) = waf
            .call(
                "CreateRuleGroup",
                json!({
                    "Name": &group_name,
                    "Scope": MONETIZED_SCOPE,
                    "Capacity": 1_000,
                    "VisibilityConfig": visibility(&group_name),
                    "MonetizationConfig": row.config.clone(),
                }),
            )
            .await;
        assert_connected_account_not_found(status, &body, &case);
        let stored = names_in_scope(&waf, "ListRuleGroups", "RuleGroups", MONETIZED_SCOPE).await;
        assert!(
            !stored.contains(&group_name),
            "case {case:?}: a refused create must store no rule group, got {stored:?}"
        );
    }
}

/// An UPDATE is a save too, so it resolves the account before it writes: the refusal is
/// the same, the stored document keeps its `LockToken` and carries no monetization
/// configuration afterwards.
#[tokio::test]
async fn an_update_naming_a_connected_account_is_refused_and_writes_nothing() {
    let waf = Waf::new();

    let acl_name = "connected-update-acl";
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": acl_name,
                "Scope": MONETIZED_SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(acl_name),
            }),
        )
        .await;
    let acl_id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let acl_lock = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    let group_name = "connected-update-group";
    let (group_id, _group_arn, group_lock) =
        create_rule_group(&waf, MONETIZED_SCOPE, group_name, 1_000, json!([])).await;

    for row in connected_account_rows() {
        let case = format!("UpdateWebACL with {}", row.case);
        let (status, body) = waf
            .call(
                "UpdateWebACL",
                json!({
                    "Name": acl_name,
                    "Scope": MONETIZED_SCOPE,
                    "Id": &acl_id,
                    "LockToken": &acl_lock,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(acl_name),
                    "MonetizationConfig": row.config.clone(),
                }),
            )
            .await;
        assert_connected_account_not_found(status, &body, &case);
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": acl_name, "Scope": MONETIZED_SCOPE, "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["LockToken"].as_str(),
            Some(acl_lock.as_str()),
            "case {case:?}: a refused update must not rotate the LockToken, got {got}"
        );
        assert!(
            got["WebACL"].get("MonetizationConfig").is_none(),
            "case {case:?}: a refused update must store no MonetizationConfig, got {got}"
        );

        let case = format!("UpdateRuleGroup with {}", row.case);
        let (status, body) = waf
            .call(
                "UpdateRuleGroup",
                json!({
                    "Name": group_name,
                    "Scope": MONETIZED_SCOPE,
                    "Id": &group_id,
                    "LockToken": &group_lock,
                    "VisibilityConfig": visibility(group_name),
                    "MonetizationConfig": row.config.clone(),
                }),
            )
            .await;
        assert_connected_account_not_found(status, &body, &case);
        let got = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": group_name, "Scope": MONETIZED_SCOPE, "Id": &group_id }),
            )
            .await;
        assert_eq!(
            got["LockToken"].as_str(),
            Some(group_lock.as_str()),
            "case {case:?}: a refused update must not rotate the LockToken, got {got}"
        );
        assert!(
            got["RuleGroup"].get("MonetizationConfig").is_none(),
            "case {case:?}: a refused update must store no MonetizationConfig, got {got}"
        );
    }
}

/// The complement, which is what makes the rule about the member and not about
/// monetization: a configuration that does not name a connected account is accepted,
/// stored and read back verbatim — for a web ACL and for a rule group.
#[tokio::test]
async fn a_configuration_naming_no_connected_account_is_stored_and_reads_back() {
    let waf = Waf::new();

    for (index, (case, config)) in unreferencing_rows().iter().enumerate() {
        let acl_name = format!("unreferencing-acl-{index}");
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": MONETIZED_SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "MonetizationConfig": config.clone(),
                }),
            )
            .await;
        let acl_id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": MONETIZED_SCOPE, "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["MonetizationConfig"],
            *config,
            "case {case:?}: the stored MonetizationConfig must be the one the save \
             carried, got {got}"
        );

        let group_name = format!("unreferencing-group-{index}");
        let created = waf
            .ok(
                "CreateRuleGroup",
                json!({
                    "Name": &group_name,
                    "Scope": MONETIZED_SCOPE,
                    "Capacity": 1_000,
                    "VisibilityConfig": visibility(&group_name),
                    "MonetizationConfig": config.clone(),
                }),
            )
            .await;
        let group_id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
            .to_string();
        let got = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": &group_name, "Scope": MONETIZED_SCOPE, "Id": &group_id }),
            )
            .await;
        assert_eq!(
            got["RuleGroup"]["MonetizationConfig"],
            *config,
            "case {case:?}: the stored MonetizationConfig must be the one the save \
             carried, got {got}"
        );
    }
}

/// The precondition is about the SETTLEMENT ACCOUNT the configuration names, not about
/// Payments ARNs in general: a save that carries one in a member with nothing to do with
/// settlement — a tag's value, a custom response body's content — names no connected
/// account, so it is created normally and reads back.
#[tokio::test]
async fn a_payments_arn_in_an_unrelated_member_names_no_connected_account() {
    let waf = Waf::new();
    let unrelated = "arn:aws:payments:us-east-1:111122223333:invoice/42";

    let acl_name = "unrelated-payments-arn-acl";
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": acl_name,
                "Scope": MONETIZED_SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(acl_name),
                "MonetizationConfig": crypto_config(),
                "Tags": [{ "Key": "owner", "Value": unrelated }],
                "CustomResponseBodies": {
                    "settled": { "ContentType": "TEXT_PLAIN", "Content": unrelated },
                },
            }),
        )
        .await;
    let acl_id = created["Summary"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a Summary.Id, got {created}"))
        .to_string();
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": acl_name, "Scope": MONETIZED_SCOPE, "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        got["WebACL"]["MonetizationConfig"],
        crypto_config(),
        "the web ACL must be stored with the configuration the save carried, got {got}"
    );
    assert_eq!(
        got["WebACL"]["CustomResponseBodies"]["settled"]["Content"].as_str(),
        Some(unrelated),
        "the unrelated member must be stored as it was written, got {got}"
    );

    let group_name = "unrelated-payments-arn-group";
    let created = waf
        .ok(
            "CreateRuleGroup",
            json!({
                "Name": group_name,
                "Scope": MONETIZED_SCOPE,
                "Capacity": 1_000,
                "VisibilityConfig": visibility(group_name),
                "MonetizationConfig": crypto_config(),
                "Tags": [{ "Key": "owner", "Value": unrelated }],
            }),
        )
        .await;
    let group_id = created["Summary"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a Summary.Id, got {created}"))
        .to_string();
    let got = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": group_name, "Scope": MONETIZED_SCOPE, "Id": &group_id }),
        )
        .await;
    assert_eq!(
        got["RuleGroup"]["MonetizationConfig"],
        crypto_config(),
        "the rule group must be stored with the configuration the save carried, got {got}"
    );
}

/// The member keeps its modelled constraint, which is a different refusal: a value
/// shorter than the `ResourceArn` minimum is refused by the request boundary as a member
/// constraint rather than by the resolution, so a caller can tell a malformed reference
/// from one that resolves to nothing.
#[tokio::test]
async fn a_reference_outside_the_members_constraint_is_a_member_refusal() {
    let waf = Waf::new();
    let acl_name = "short-connected-account-acl";
    let (status, body) = waf
        .call(
            "CreateWebACL",
            json!({
                "Name": acl_name,
                "Scope": MONETIZED_SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(acl_name),
                "MonetizationConfig": {
                    "ConnectedAccountConfigs": [
                        { "ConnectedAccountArn": "arn:aws:payments" },
                    ],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a value under the member's minimum length to be refused, got status \
         {status} and body {body}"
    );
    let code = body["__type"].as_str().unwrap_or_default();
    assert!(
        !code.ends_with(NONEXISTENT_ITEM_CODE),
        "expected the member-constraint refusal rather than the resolution's, got {body}"
    );
    let stored = names_in_scope(&waf, "ListWebACLs", "WebACLs", MONETIZED_SCOPE).await;
    assert!(
        !stored.contains(&acl_name.to_string()),
        "a refused create must store no web ACL, got {stored:?}"
    );
}

/// The ORDER the monetization judgements are taken in: the namespace comes first.
/// Monetization is not served regionally at all, so a regional save naming a connected
/// account is refused for the NAMESPACE — the account of a feature that does not exist
/// there is never resolved — and nothing is stored either way.
#[tokio::test]
async fn a_regional_save_naming_a_connected_account_is_refused_for_the_namespace() {
    let waf = Waf::new();

    for (index, row) in connected_account_rows().iter().enumerate() {
        let acl_name = format!("regional-connected-acl-{index}");
        let case = format!("REGIONAL CreateWebACL with {}", row.case);
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": REGIONAL_SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "MonetizationConfig": row.config.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the save to be refused with 400, got status {status} \
             and body {body}"
        );
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.ends_with(INVALID_OPERATION_CODE),
            "case {case:?}: expected the error code {INVALID_OPERATION_CODE}, got \
             {code:?} in body {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(MONETIZATION_NOT_AVAILABLE),
            "case {case:?}: expected WAF's monetization sentence for the regional \
             namespace, got {:?}",
            body["Message"]
        );
        let stored = names_in_scope(&waf, "ListWebACLs", "WebACLs", REGIONAL_SCOPE).await;
        assert!(
            !stored.contains(&acl_name),
            "case {case:?}: a refused create must store no web ACL, got {stored:?}"
        );
    }
}

/// A reference that is well formed but names a partition connected accounts are not
/// available in is the PARAMETER refusal, not the resolution's.
///
/// `ConnectedAccountConfig`'s own doc states both halves: "connected accounts are
/// available only in the commercial Amazon Web Services partition", and a request that
/// references one is rejected "with `WAFInvalidParameterException` if the ARN is invalid
/// or the partition is unsupported" — a different answer from the
/// `WAFNonexistentItemException` an addressable account that does not exist earns, so a
/// caller can tell a reference WAF cannot address from one it resolved and did not find.
///
/// The rows are the partitions the member's own pattern admits besides the commercial
/// one (`^arn:aws(-[a-z]+)*:payments::…`), on a reference that is otherwise exactly the
/// form the pattern pins.
#[tokio::test]
async fn a_reference_outside_the_commercial_partition_is_a_parameter_refusal() {
    let waf = Waf::new();

    for (index, partition) in ["aws-us-gov", "aws-cn", "aws-iso-b"].iter().enumerate() {
        let acl_name = format!("partition-connected-acl-{index}");
        let case = format!("CreateWebACL naming the {partition} partition");
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": MONETIZED_SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "MonetizationConfig": {
                        "ConnectedAccountConfigs": [{
                            "ConnectedAccountArn": connected_account_arn(
                                partition,
                                "000000000000",
                                "acct-ABC123",
                            ),
                            "PaymentNetworks": [connected_account_network("BASE", "0.01")],
                        }],
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the save to be refused with 400, got status {status} \
             and body {body}"
        );
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.ends_with("WAFInvalidParameterException"),
            "case {case:?}: expected the parameter refusal for an unsupported partition, \
             got {code:?} in body {body}"
        );
        let stored = names_in_scope(&waf, "ListWebACLs", "WebACLs", MONETIZED_SCOPE).await;
        assert!(
            !stored.contains(&acl_name),
            "case {case:?}: a refused create must store no web ACL, got {stored:?}"
        );
    }
}

/// A `ConnectedAccountConfigs` the model does not allow is refused as the member
/// constraint it is, before any resolution and before anything is written.
///
/// The rows span the bounds the model declares for the list and its entries: no entry at
/// all and two entries against `length(1..1)` ("currently supports exactly one connected
/// account"), an entry that leaves out either `@required` member, and a `PaymentNetworks`
/// that is empty or holds three networks against its `length(1..2)` ("you can specify 1
/// to 2 networks").
#[tokio::test]
async fn a_connected_account_configuration_the_model_does_not_allow_is_refused() {
    let waf = Waf::new();
    let arn = connected_account_arn("aws", "000000000000", "acct-ABC123");
    let network = connected_account_network("BASE", "0.01");
    let entry = json!({ "ConnectedAccountArn": &arn, "PaymentNetworks": [&network] });

    // `(case, the ConnectedAccountConfigs the save carries)`.
    let cases: Vec<(&str, Value)> = vec![
        ("no entry at all", json!([])),
        ("two entries", json!([&entry, &entry])),
        (
            "an entry naming no account",
            json!([{ "PaymentNetworks": [&network] }]),
        ),
        (
            "an entry pricing no network",
            json!([{ "ConnectedAccountArn": &arn }]),
        ),
        (
            "an entry whose network list is empty",
            json!([{ "ConnectedAccountArn": &arn, "PaymentNetworks": [] }]),
        ),
        (
            "an entry pricing three networks",
            json!([{
                "ConnectedAccountArn": &arn,
                "PaymentNetworks": [
                    connected_account_network("BASE", "0.01"),
                    connected_account_network("SOLANA", "0.02"),
                    connected_account_network("BASE", "0.03"),
                ],
            }]),
        ),
    ];

    for (index, (case, configs)) in cases.iter().enumerate() {
        let acl_name = format!("disallowed-connected-acl-{index}");
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": MONETIZED_SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "MonetizationConfig": { "ConnectedAccountConfigs": configs },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the member-constraint refusal, got status {status} \
             and body {body}"
        );
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.ends_with("ValidationException"),
            "case {case:?}: expected the request validator's envelope, got {code:?} in \
             body {body}"
        );
        let stored = names_in_scope(&waf, "ListWebACLs", "WebACLs", MONETIZED_SCOPE).await;
        assert!(
            !stored.contains(&acl_name),
            "case {case:?}: a refused create must store no web ACL, got {stored:?}"
        );
    }
}
