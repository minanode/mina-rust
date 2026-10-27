//! The public GraphQL policy (`--public-graphql`): what a node whose GraphQL
//! port anyone can reach answers. It runs after the admission limits and
//! before Juniper, so a refused request reaches no resolver and no RPC.
//!
//! - One query per request: no batches, mutations or subscriptions.
//! - Only the fields of [`ALLOWED`], found by walking the parsed document
//!   with the schema's types, fragments expanded. `__typename` is allowed,
//!   `__schema` and `__type` (introspection) are not. Fields the schema does
//!   not have are left to Juniper's validation.
//! - `bestChain` needs `maxLength` in 1..=[`MAX_BEST_CHAIN`]; the pools need
//!   `publicKey` or `hashes`/`ids`. Every supplied list is capped at
//!   [`MAX_POOL_IDS`], even when a public key is supplied.
//!   `verificationReceipts` takes at most [`MAX_RECEIPT_HASHES`] state
//!   hashes and at most one selection per request (aliases/fragments included),
//!   so it costs at most one lookup (30). The resolver also caps each list.
//! - At most [`MAX_DEPTH`] levels of fields, [`MAX_SELECTIONS`] expanded
//!   selections, [`MAX_ROOT_FIELDS`] root fields, and a cost of at most
//!   [`MAX_COST`]: an account lookup costs 10, a block 20, a pool or
//!   transaction status lookup 30, `stakingActive` (the node status) 30,
//!   a staking ledger lookup (`epochDelegateAccount`, `lastEpochDelegators`)
//!   10 more than its accounts, a proof or
//!   verification key 1, each multiplied by the lists around it (the
//!   `maxLength` of `bestChain`, [`BLOCK_LIST`] for a block's commands,
//!   fee transfers and SNARK jobs, [`USER_POOL_LIST`] and
//!   [`ZKAPP_POOL_LIST`] for the pools, [`OTHER_LIST`]
//!   for any other list). The grant frontend's `RecentBlocks` (20 blocks
//!   with their coinbase receiver) costs 600.
//!
//! These are acceptance limits from R30's proposal, not measured CPU
//! bounds; Caddy still limits the rate. `accounts` and `tokenOwner` are
//! private-only because their lookup can scan the entire ledger.
use std::collections::HashMap;

use juniper::{
    http::{GraphQLBatchRequest, GraphQLRequest},
    meta::MetaType,
    parser::parse_document_source,
    DefaultScalarValue, Definition, InputValue, OperationType, Selection,
};
use serde_json::{json, Value};

use super::graphql::Schema;

pub(super) const MAX_BEST_CHAIN: i64 = 20;
pub(super) const MAX_POOL_IDS: usize = 10;
pub(super) const MAX_RECEIPT_HASHES: usize =
    crate::graphql::verification::MAX_PUBLIC_RECEIPT_HASHES;
pub(super) const MAX_DEPTH: usize = 12;
pub(super) const MAX_SELECTIONS: usize = 300;
pub(super) const MAX_ROOT_FIELDS: usize = 10;
pub(super) const MAX_COST: u64 = 1000;
const BLOCK_LIST: u64 = 128;
const USER_POOL_LIST: u64 = 50;
const ZKAPP_POOL_LIST: u64 = 10;
const OTHER_LIST: u64 = 8;

/// The fields a public request may select, by type. `None`: every field of
/// the type.
const ALLOWED: &[(&str, Option<&[&str]>)] = &[
    (
        "Query",
        Some(&[
            "syncStatus",
            "networkID",
            "genesisConstants",
            "daemonStatus",
            "account",
            "bestChain",
            "block",
            "genesisBlock",
            "transactionStatus",
            "pooledUserCommands",
            "pooledZkappCommands",
            "verificationReceipts",
        ]),
    ),
    (
        "DaemonStatus",
        Some(&[
            "stateHash",
            "blockchainLength",
            "syncStatus",
            "chainId",
            "consensusConfiguration",
        ]),
    ),
    ("ConsensusConfiguration", None),
    ("GraphQLGenesisConstants", None),
    (
        "Account",
        Some(&[
            "publicKey",
            "tokenId",
            "token",
            "tokenSymbol",
            "balance",
            "nonce",
            "inferredNonce",
            "delegate",
            "receiptChainHash",
            "delegateAccount",
            "epochDelegateAccount",
            "lastEpochDelegators",
            "stakingActive",
            "votingFor",
            "timing",
            "permissions",
            "zkappState",
            "verificationKey",
            "actionState",
            "provedState",
            "zkappUri",
        ]),
    ),
    ("GraphQLBalance", None),
    ("AccountTiming", None),
    ("GraphQLPermissions", None),
    ("GraphQLSetVerificationKey", None),
    ("GraphQLVerificationKey", None),
    (
        "Block",
        Some(&[
            "creator",
            "creatorAccount",
            "winnerAccount",
            "stateHash",
            "stateHashField",
            "protocolState",
            "transactions",
            "commandTransactionCount",
            "snarkJobs",
        ]),
    ),
    ("ProtocolState", None),
    ("BlockchainState", None),
    ("ConsensusState", None),
    ("GraphQLEpochData", None),
    ("epochLedger", None),
    ("Transactions", None),
    ("FeeTransfer", None),
    ("CompletedWork", None),
    ("UserCommand", None),
    ("UserCommandPayment", None),
    ("UserCommandDelegation", None),
    ("ZkappCommandResult", None),
    ("ZkappCommandFailureReason", None),
    ("GraphQLZkappCommand", None),
    ("GraphQLFeePayer", None),
    ("GraphQLFeePayerBody", None),
    ("GraphQLAccountUpdate", None),
    ("GraphQLAuthorization", None),
    ("GraphQLAccountUpdateBody", None),
    ("GraphQLAuthorizationKind", None),
    ("GraphQLMayUseToken", None),
    ("GraphQLPreconditions", None),
    ("GraphQLPreconditionsNetworkBounds", None),
    ("GraphQLPreconditionsAccount", None),
    ("GraphQLPreconditionsNetwork", None),
    ("GraphQLPreconditionsNetworkEpochData", None),
    ("GraphQLPreconditionsNetworkLedger", None),
    ("GraphQLBalanceChange", None),
    ("GraphQLAccountUpdateUpdate", None),
    ("GraphQLAccountUpdateUpdatePermissions", None),
    ("GraphQLSetVerificationKeyPermissions", None),
    // Named field by field: the public receipt types are already a fixed
    // projection, and a field added to them is not published by accident.
    (
        "PublicVerificationReceipt",
        Some(&[
            "stateHash",
            "height",
            "network",
            "arrival",
            "applied",
            "blockProof",
            "deltaChain",
            "bodyReference",
            "verifierKey",
        ]),
    ),
    ("PublicVerificationCheck", Some(&["status", "elapsedMs"])),
];

/// A refused request: HTTP 400 with this message and code.
#[derive(Debug, PartialEq)]
pub(super) struct Refusal {
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(super) fn to_json(&self) -> Value {
        json!({
            "errors": [{ "message": self.message, "extensions": { "code": self.code } }]
        })
    }
}

const NOT_ALLOWED: &str = "PUBLIC_GRAPHQL_NOT_ALLOWED";
const TOO_COMPLEX: &str = "PUBLIC_GRAPHQL_TOO_COMPLEX";
const BAD_ARGUMENT: &str = "PUBLIC_GRAPHQL_BAD_ARGUMENT";

pub(super) fn check(request: &GraphQLBatchRequest, schema: &Schema) -> Result<(), Refusal> {
    let GraphQLBatchRequest::Single(request) = request else {
        return Err(Refusal::new(
            NOT_ALLOWED,
            "This public endpoint takes one request at a time, not batches",
        ));
    };
    check_one(request, schema)
}

fn check_one(request: &GraphQLRequest, schema: &Schema) -> Result<(), Refusal> {
    // The limits ran first, so the document parses; one that doesn't is
    // left to Juniper.
    let Ok(document) = parse_document_source(&request.query, &schema.schema) else {
        return Ok(());
    };
    let mut operations = Vec::new();
    let mut fragments = HashMap::new();
    for definition in &document {
        match definition {
            Definition::Operation(operation) => operations.push(&operation.item),
            Definition::Fragment(fragment) => {
                fragments.insert(
                    fragment.item.name.item,
                    (
                        fragment.item.type_condition.item,
                        fragment.item.selection_set.as_slice(),
                    ),
                );
            }
        }
    }
    let [operation] = operations.as_slice() else {
        return Err(Refusal::new(
            NOT_ALLOWED,
            "This public endpoint takes documents with exactly one operation",
        ));
    };
    if operation.operation_type != OperationType::Query {
        return Err(Refusal::new(
            NOT_ALLOWED,
            "This public endpoint is read-only: only queries are allowed",
        ));
    }
    let mut walker = Walker {
        schema,
        fragments,
        variables: request
            .variables
            .as_ref()
            .and_then(InputValue::to_object_value)
            .map(|v| v.into_iter().collect())
            .unwrap_or_default(),
        active: Vec::new(),
        selections: 0,
        root_fields: 0,
        receipt_selections: 0,
    };
    // Explicit values (including null) override operation defaults. The cost
    // and pool bounds must see the same effective values as the resolver.
    for (name, definition) in operation
        .variable_definitions
        .iter()
        .flat_map(|v| &v.item.items)
    {
        if let Some(value) = &definition.default_value {
            walker.variables.entry(name.item).or_insert(&value.item);
        }
    }
    let cost = walker.walk("Query", &operation.selection_set, 1)?;
    if cost > MAX_COST {
        return Err(Refusal::new(
            TOO_COMPLEX,
            format!("Query cost {cost} exceeds {MAX_COST}"),
        ));
    }
    Ok(())
}

type Fragments<'d> = HashMap<&'d str, (&'d str, &'d [Selection<'d>])>;

struct Walker<'d> {
    schema: &'d Schema,
    fragments: Fragments<'d>,
    variables: HashMap<&'d str, &'d InputValue>,
    active: Vec<&'d str>,
    selections: usize,
    root_fields: usize,
    receipt_selections: usize,
}

impl<'d> Walker<'d> {
    /// The cost of a selection set on `type_name`, checking its fields.
    fn walk(
        &mut self,
        type_name: &str,
        selections: &'d [Selection<'d>],
        depth: usize,
    ) -> Result<u64, Refusal> {
        if depth > MAX_DEPTH {
            return Err(Refusal::new(
                TOO_COMPLEX,
                format!("At most {MAX_DEPTH} levels of fields"),
            ));
        }
        // TypeModifier is not exported by Juniper. Compare modifiers to a
        // constructed NonNull: every other modifier is a list (of any size).
        let non_null = juniper::Type::<&str>::nullable("").wrap_non_null();
        let mut cost = 0u64;
        for selection in selections {
            self.selections += 1;
            if self.selections > MAX_SELECTIONS {
                return Err(Refusal::new(
                    TOO_COMPLEX,
                    format!("At most {MAX_SELECTIONS} selections, fragments expanded"),
                ));
            }
            let part = match selection {
                Selection::Field(field) => {
                    let field = &field.item;
                    let name = field.name.item;
                    if depth == 1 {
                        self.root_fields += 1;
                        if self.root_fields > MAX_ROOT_FIELDS {
                            return Err(Refusal::new(
                                TOO_COMPLEX,
                                format!("At most {MAX_ROOT_FIELDS} root fields per request"),
                            ));
                        }
                    }
                    if name == "__typename" {
                        continue;
                    }
                    if name.starts_with("__") {
                        return Err(Refusal::new(
                            NOT_ALLOWED,
                            "Introspection is not allowed on this public endpoint",
                        ));
                    }
                    // Juniper refuses what the schema does not have.
                    let Some(meta) = self
                        .schema
                        .schema
                        .concrete_type_by_name(type_name)
                        .and_then(|t: &MetaType| t.field_by_name(name))
                    else {
                        continue;
                    };
                    if !allowed(type_name, name) {
                        return Err(Refusal::new(
                            NOT_ALLOWED,
                            format!(
                                "`{type_name}.{name}` is not available on this public endpoint"
                            ),
                        ));
                    }
                    let argument = |wanted: &str| {
                        field
                            .arguments
                            .iter()
                            .flat_map(|a| &a.item.items)
                            .find(|(n, _)| n.item == wanted)
                            .map(|(_, v)| &v.item)
                    };
                    let (call, list) = match (type_name, name) {
                        ("Query", "bestChain") => {
                            let length = self.int(argument("maxLength"));
                            match length.filter(|n| (1..=MAX_BEST_CHAIN).contains(n)) {
                                Some(n) => (0, n as u64),
                                None => {
                                    return Err(Refusal::new(
                                        BAD_ARGUMENT,
                                        format!(
                                            "bestChain needs maxLength from 1 to {MAX_BEST_CHAIN}"
                                        ),
                                    ))
                                }
                            }
                        }
                        ("Query", "pooledUserCommands" | "pooledZkappCommands") => {
                            let by_key = self.given(argument("publicKey"));
                            let ids = [argument("hashes"), argument("ids")]
                                .map(|v| self.list_len(v))
                                .into_iter()
                                .flatten()
                                .max();
                            if ids.is_some_and(|n| n > MAX_POOL_IDS) || (!by_key && ids.is_none()) {
                                return Err(Refusal::new(
                                    BAD_ARGUMENT,
                                    format!("{name} needs publicKey or hashes/ids; each list is limited to {MAX_POOL_IDS}"),
                                ));
                            }
                            let list = if name == "pooledUserCommands" {
                                USER_POOL_LIST
                            } else {
                                ZKAPP_POOL_LIST
                            };
                            (30, list)
                        }
                        ("Query", "transactionStatus") => (30, 1),
                        ("Query", "verificationReceipts") => {
                            // Count every occurrence, just like root fields: aliases,
                            // repeated/nested fragments and directives cannot create
                            // another resolver call after admission. With one selection
                            // and one bounded list, the whole request has <=20 hashes
                            // and <=1 lookup, before any resolver or RPC runs.
                            self.receipt_selections += 1;
                            if self.receipt_selections > 1 {
                                return Err(Refusal::new(
                                    BAD_ARGUMENT,
                                    "At most one verificationReceipts selection per request, including aliases and fragments",
                                ));
                            }
                            let hashes = self.list_len(argument("stateHashes"));
                            if hashes.is_some_and(|n| n > MAX_RECEIPT_HASHES) {
                                return Err(Refusal::new(
                                    BAD_ARGUMENT,
                                    format!("verificationReceipts takes at most {MAX_RECEIPT_HASHES} stateHashes"),
                                ));
                            }
                            // One lookup answers every hash.
                            (30, 1)
                        }
                        ("Account", "stakingActive") => (30, 1),
                        ("Account", "epochDelegateAccount") => (10, 1),
                        ("Account", "lastEpochDelegators") => {
                            (10, mina_node::rpc::MAX_LAST_EPOCH_DELEGATORS as u64)
                        }
                        ("Transactions", "userCommands" | "zkappCommands" | "feeTransfer")
                        | ("Block", "snarkJobs") => (0, BLOCK_LIST),
                        // Juniper 0.17's is_list() misses outer NonNull lists.
                        // Count every list wrapper, including nested lists.
                        _ => (
                            0,
                            meta.field_type
                                .modifiers()
                                .iter()
                                .fold(1u64, |n, modifier| {
                                    if Some(modifier) == non_null.modifier() {
                                        n
                                    } else {
                                        n.saturating_mul(OTHER_LIST)
                                    }
                                }),
                        ),
                    };
                    let child_type = meta.field_type.innermost_name();
                    let element = match (type_name, name, child_type) {
                        (_, _, "Account") => 10,
                        (_, _, "Block") => 20,
                        ("GraphQLAuthorization", "proof", _)
                        | ("GraphQLVerificationKey", "verificationKey" | "data", _) => 1,
                        _ => 0,
                    };
                    let children = match &field.selection_set {
                        Some(children) => self.walk(child_type, children, depth + 1)?,
                        None => 0,
                    };
                    list.saturating_mul(element + children).saturating_add(call)
                }
                Selection::InlineFragment(fragment) => {
                    let on = fragment
                        .item
                        .type_condition
                        .as_ref()
                        .map_or(type_name, |t| t.item);
                    self.walk(on, &fragment.item.selection_set, depth)?
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.item.name.item;
                    let Some(&(on, children)) = self.fragments.get(name) else {
                        continue; // Juniper refuses it.
                    };
                    if self.active.contains(&name) {
                        return Err(Refusal::new(TOO_COMPLEX, "Cyclic fragment"));
                    }
                    self.active.push(name);
                    let part = self.walk(on, children, depth)?;
                    self.active.pop();
                    part
                }
            };
            cost = cost.saturating_add(part);
        }
        Ok(cost)
    }

    /// An integer argument, literal or variable.
    fn int(&self, value: Option<&InputValue>) -> Option<i64> {
        match self.resolve(value?)?.as_scalar()? {
            DefaultScalarValue::Int(n) => Some(i64::from(*n)),
            _ => None,
        }
    }

    /// Whether an argument has a value other than null.
    fn given(&self, value: Option<&InputValue>) -> bool {
        value
            .and_then(|v| self.resolve(v))
            .is_some_and(|v| !v.is_null())
    }

    /// The length of a list argument; a single value is a list of one, as
    /// GraphQL coerces it.
    fn list_len(&self, value: Option<&InputValue>) -> Option<usize> {
        let value = self.resolve(value?)?;
        match value {
            InputValue::Null => None,
            InputValue::List(items) => Some(items.len()),
            _ => Some(1),
        }
    }

    fn resolve<'v>(&'v self, value: &'v InputValue) -> Option<&'v InputValue> {
        match value {
            InputValue::Variable(name) => self.variables.get(name.as_str()).copied(),
            value => Some(value),
        }
    }
}

fn allowed(type_name: &str, field: &str) -> bool {
    ALLOWED
        .iter()
        .find(|(name, _)| *name == type_name)
        .is_some_and(|(_, fields)| fields.is_none_or(|fields| fields.contains(&field)))
}
