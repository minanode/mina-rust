//! LN1 light-node probe.
//!
//! A minimal light client. It trusts only a network's blockchain verification key
//! and genesis state hash, and treats every data source as untrusted:
//!
//! 1. Fetch the best tip with its proof chain from one peer (`get_best_tip`), or
//!    from a Rust node's light endpoint (`GET /light/best-tip`, LN2).
//! 2. Verify the tip's blockchain SNARK, pin its genesis state hash, and check the
//!    root-to-tip chain of state body hashes.
//! 3. Read one account with its Merkle path, from a peer (`answer_sync_ledger_query`
//!    against the root snarked or an epoch ledger) and from a GraphQL endpoint
//!    (the tip's snarked ledger, and the tip's staged ledger), and check each
//!    path against a ledger hash taken from the verified state.
//!
//! It prints what it verified, per-phase timings, peak RSS and the bytes it
//! downloaded: whole RPC frames on the libp2p stream and HTTP response bodies.
//! Transport framing (noise, yamux, TLS, TCP) is not counted.

mod merkle;
mod rpc;

use std::{
    str::FromStr,
    time::{Duration, Instant, SystemTime},
};

use base64::Engine;
use binprot::{BinProtRead, BinProtWrite};
use clap::{Parser, ValueEnum};
use ledger::{Account, AccountIndex, Address};
use libp2p::{futures::StreamExt, identity::Keypair, swarm::SwarmEvent, Multiaddr};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    list::List,
    rpc::{AnswerSyncLedgerQueryV2, GetBestTipV2},
    v2::{
        LedgerHash, MinaBaseAccountBinableArgStableV2, MinaBaseStateBodyHashStableV1,
        MinaBlockHeaderStableV2, MinaLedgerSyncLedgerAnswerStableV2 as Answer,
        MinaLedgerSyncLedgerQueryStableV1 as Query, MinaStateProtocolStateValueStableV2, StateHash,
    },
};
use mina_web_verifier::Verifier;
use serde_json::{json, Value};

use crate::merkle::{empty_hashes, implied_root, subtree_rows};

/// Ledger depth on Mesa devnet and mainnet.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L1>
const LEDGER_DEPTH: usize = 35;
/// `What_contents` answers cover subtrees of this height (64 accounts).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_ledger/sync_ledger.ml#L22-L50>
const ACCOUNT_SUBTREE_HEIGHT: usize = 6;
/// Best-tip proofs cover `k` blocks unless the root is genesis.
const K: usize = 290;

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum Network {
    Devnet,
    Mainnet,
}

struct NetworkInfo {
    chain_id: String,
    /// The `genesis_state_hash` that Mesa blocks carry.
    genesis_state_hash: &'static str,
    seeds: &'static [&'static str],
    graphql: &'static str,
    vk: &'static str,
}

impl Network {
    fn info(self) -> NetworkInfo {
        match self {
            Network::Devnet => NetworkInfo {
                chain_id: mina_core::DEVNET_CHAIN_ID.to_hex(),
                genesis_state_hash: "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ",
                seeds: &[
                    "/dns4/seed-1.devnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs",
                    "/dns4/seed-2.devnet.gcp.o1test.net/tcp/32012/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag",
                    "/dns4/seed-3.devnet.gcp.o1test.net/tcp/32013/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv",
                ],
                graphql: "https://mina-devnet-graphql.aurowallet.com/graphql",
                vk: "devnet-mesa-blockchain-vk.json",
            },
            Network::Mainnet => NetworkInfo {
                chain_id: mina_core::MAINNET_CHAIN_ID.to_hex(),
                genesis_state_hash: "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7",
                seeds: &[
                    "/dns4/seed-1.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWCa1d7G3SkRxy846qTvdAFX69NnoYZ32orWVLqJcDVGHW",
                    "/dns4/seed-4.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWEdBiTUQqxp3jeuWaZkwiSNcFxC6d6Tdq7u2Lf2ZD2Q6X",
                ],
                graphql: "https://mina-mainnet-graphql.aurowallet.com/graphql",
                vk: "mainnet-mesa-blockchain-vk.json",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum LedgerChoice {
    /// The root block's snarked ledger (OCaml serves it; k blocks behind the tip).
    Root,
    /// The tip's staking epoch ledger.
    Staking,
    /// The tip's next epoch ledger.
    Next,
    /// The tip's snarked ledger (our node serves it; OCaml does not).
    TipSnarked,
}

/// A ledger the verified tip names, as `GET /light/account?ledger=` takes it.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum LightLedger {
    Staged,
    Snarked,
    Staking,
    Next,
}

impl LightLedger {
    fn name(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Snarked => "snarked",
            Self::Staking => "staking",
            Self::Next => "next",
        }
    }

    /// The ledger hash, read from the verified tip itself.
    fn hash_in(self, tip: &MinaBlockHeaderStableV2) -> LedgerHash {
        let cs = &tip.protocol_state.body.consensus_state;
        match self {
            Self::Staged => staged_ledger(tip),
            Self::Snarked => snarked_ledger(tip),
            Self::Staking => cs.staking_epoch_data.ledger.hash.clone(),
            Self::Next => cs.next_epoch_data.ledger.hash.clone(),
        }
    }

    fn field(self) -> &'static str {
        match self {
            Self::Staged => {
                "protocol_state.body.blockchain_state.staged_ledger_hash.non_snark.ledger_hash"
            }
            Self::Snarked => {
                "protocol_state.body.blockchain_state.ledger_proof_statement.target.first_pass_ledger"
            }
            Self::Staking => "protocol_state.body.consensus_state.staking_epoch_data.ledger.hash",
            Self::Next => "protocol_state.body.consensus_state.next_epoch_data.ledger.hash",
        }
    }
}

#[derive(Parser)]
#[command(about = "LN1: verify a best tip and one account's Merkle path from untrusted sources")]
struct Args {
    #[arg(long, value_enum, default_value_t = Network::Devnet)]
    network: Network,
    /// Peers to dial for RPCs; defaults to the network's seeds.
    #[arg(long)]
    peer: Vec<String>,
    /// Take the tip from this Rust node's `GET /light/best-tip` instead of a
    /// peer, and read the account from its `GET /light/account` (LN2). Peer
    /// ledger queries are skipped in this mode.
    #[arg(long)]
    http: Option<String>,
    /// With --http, the ledgers to check the account in (repeatable).
    #[arg(long, value_enum, default_values_t = [LightLedger::Staged, LightLedger::Snarked, LightLedger::Staking, LightLedger::Next])]
    light_ledger: Vec<LightLedger>,
    /// Account public key (B62...), or `tip-winner` for the verified tip's block
    /// stake winner, which is in the staking epoch ledger by construction.
    #[arg(long)]
    account: String,
    /// Account index in the ledger. Without it, the GraphQL endpoint is asked
    /// for a hint; a wrong hint fails the check rather than fooling it.
    #[arg(long)]
    index: Option<u64>,
    /// Which verified ledger hash to read the account against over p2p.
    #[arg(long, value_enum, default_value_t = LedgerChoice::Root)]
    ledger: LedgerChoice,
    /// Levels per `What_child_hashes` query (servers clamp to 8).
    #[arg(long, default_value_t = 8)]
    subtree_depth: usize,
    /// GraphQL endpoint; defaults to the public Auro endpoint of the network.
    #[arg(long)]
    graphql: Option<String>,
    /// Don't use GraphQL at all (needs --index).
    #[arg(long)]
    no_graphql: bool,
    /// Also verify the root block's proof (the tip's proof already covers it).
    #[arg(long)]
    verify_root: bool,
    /// Blockchain verification key JSON; defaults to `data/vk/<network>-mesa-blockchain-vk.json`.
    #[arg(long)]
    vk: Option<String>,
    /// Write a JSON report here.
    #[arg(long)]
    report: Option<String>,
    /// Give up on the peer after this many seconds.
    #[arg(long, default_value_t = 120)]
    timeout: u64,
}

/// Peak resident set size so far, KiB.
fn vm_hwm_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

struct Phases {
    start: Instant,
    rows: Vec<Value>,
    last: Instant,
}

impl Phases {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            rows: Vec::new(),
            last: now,
        }
    }

    fn mark(&mut self, name: &str) {
        let now = Instant::now();
        let row = json!({
            "phase": name,
            "ms": now.duration_since(self.last).as_millis() as u64,
            "peak_rss_kib": vm_hwm_kib(),
        });
        println!(
            "  [{name}] {} ms, peak RSS {} MiB",
            row["ms"],
            vm_hwm_kib() / 1024
        );
        self.rows.push(row);
        self.last = now;
    }
}

struct Graphql {
    url: String,
    client: reqwest::blocking::Client,
    bytes: u64,
}

impl Graphql {
    fn query(&mut self, query: &str) -> Result<Value, String> {
        let body = self
            .client
            .post(&self.url)
            .json(&json!({ "query": query }))
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| format!("GraphQL: {e}"))?;
        self.bytes += body.len() as u64;
        let v: Value = serde_json::from_slice(&body).map_err(|e| format!("GraphQL JSON: {e}"))?;
        if let Some(errors) = v.get("errors") {
            return Err(format!("GraphQL errors: {errors}"));
        }
        Ok(v["data"].clone())
    }
}

fn http_get(
    client: &reqwest::blocking::Client,
    url: &str,
    bytes: &mut u64,
) -> Result<Vec<u8>, String> {
    let body = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("GET {url}: {e}"))?;
    *bytes += body.len() as u64;
    Ok(body.to_vec())
}

fn encoded_len<T: BinProtWrite>(value: &T) -> usize {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).expect("encode");
    bytes.len()
}

fn fp_of(hash: &LedgerHash) -> Fp {
    hash.to_field().expect("ledger hash is a field element")
}

fn snarked_ledger(header: &MinaBlockHeaderStableV2) -> LedgerHash {
    header
        .protocol_state
        .body
        .blockchain_state
        .ledger_proof_statement
        .target
        .first_pass_ledger
        .clone()
}

fn staged_ledger(header: &MinaBlockHeaderStableV2) -> LedgerHash {
    header
        .protocol_state
        .body
        .blockchain_state
        .staged_ledger_hash
        .non_snark
        .ledger_hash
        .clone()
}

/// Parse a GraphQL Merkle path (`[{left, right}]`, leaf first) into siblings by
/// height. `left` set is OCaml's `` `Left sibling ``: the path node is the left
/// child and the sibling is on its right.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/merkle_path.ml#L32-L42>
fn graphql_path(path: &Value) -> Result<(Vec<Fp>, u64), String> {
    let mut siblings = Vec::new();
    let mut index = 0u64;
    for (height, elem) in path
        .as_array()
        .ok_or("merklePath is not a list")?
        .iter()
        .enumerate()
    {
        let (sibling, is_right_child) = match (elem["left"].as_str(), elem["right"].as_str()) {
            (Some(l), None) => (l, false),
            (None, Some(r)) => (r, true),
            _ => return Err(format!("bad merklePath element {elem}")),
        };
        siblings.push(Fp::from_str(sibling).map_err(|_| format!("bad field {sibling}"))?);
        if is_right_child {
            index |= 1 << height;
        }
    }
    Ok((siblings, index))
}

fn describe(account: &Account) -> Value {
    json!({
        "public_key": account.public_key.into_address(),
        "balance_nanomina": account.balance.as_u64(),
        "nonce": account.nonce.as_u32(),
        "has_zkapp": account.zkapp.is_some(),
    })
}

/// Walk `answer_sync_ledger_query` down to one account and check its path.
async fn p2p_account(
    swarm: &mut libp2p::Swarm<rpc::RpcBehaviour>,
    conn: &mut rpc::RpcConn,
    ledger_hash: &LedgerHash,
    index: u64,
    public_key: &str,
    subtree_depth: usize,
) -> Result<Value, String> {
    let mut siblings = vec![Fp::from(0u64); LEDGER_DEPTH];
    let contents_depth = LEDGER_DEPTH - ACCOUNT_SUBTREE_HEIGHT;
    let mut depth = 0;
    let mut queries = 0;
    while depth < contents_depth {
        let k = subtree_depth.min(contents_depth - depth);
        let addr = if depth == 0 {
            Address::root()
        } else {
            Address::from_index(AccountIndex(index >> (LEDGER_DEPTH - depth)), depth)
        };
        let query = (
            ledger_hash.0.clone(),
            Query::WhatChildHashes(addr.into(), (k as u64).into()),
        );
        let answer = rpc::drive(swarm, conn.call::<AnswerSyncLedgerQueryV2>(&query))
            .await?
            .0
            .map_err(|e| format!("What_child_hashes at depth {depth}: {e}"))?;
        queries += 1;
        let Answer::ChildHashesAre(hashes) = answer else {
            return Err(format!("unexpected answer at depth {depth}"));
        };
        let row: Vec<Fp> = hashes.iter().map(fp_of).collect();
        if row.len() != 1 << k {
            return Err(format!(
                "expected {} hashes at depth {depth}, got {}",
                1 << k,
                row.len()
            ));
        }
        // rows[i] holds the nodes at depth `depth + k - i`, height `35 - depth - k + i`.
        let bottom_height = LEDGER_DEPTH - depth - k;
        let rows = subtree_rows(row, bottom_height);
        for (i, nodes) in rows.iter().take(k).enumerate() {
            let height = bottom_height + i;
            let local = ((index >> height) & ((1 << (k - i)) - 1)) as usize;
            siblings[height] = nodes[local ^ 1];
        }
        depth += k;
    }

    let addr = Address::from_index(
        AccountIndex(index >> ACCOUNT_SUBTREE_HEIGHT),
        contents_depth,
    );
    let answer = rpc::drive(
        swarm,
        conn.call::<AnswerSyncLedgerQueryV2>(&(
            ledger_hash.0.clone(),
            Query::WhatContents(addr.into()),
        )),
    )
    .await?
    .0
    .map_err(|e| format!("What_contents: {e}"))?;
    queries += 1;
    let Answer::ContentsAre(accounts) = answer else {
        return Err("unexpected answer to What_contents".into());
    };
    let empty = empty_hashes(LEDGER_DEPTH);
    let mut leaves = vec![empty[0]; 1 << ACCOUNT_SUBTREE_HEIGHT];
    let offset = (index & ((1 << ACCOUNT_SUBTREE_HEIGHT) - 1)) as usize;
    let mut mine = None;
    for (i, acc) in accounts.iter().enumerate().take(leaves.len()) {
        let account = Account::try_from(acc).map_err(|e| format!("account {i}: {e:?}"))?;
        leaves[i] = account.hash();
        if i == offset {
            mine = Some(account);
        }
    }
    let account = mine.ok_or("no account at that index")?;
    if account.public_key.into_address() != public_key {
        return Err(format!(
            "account at index {index} is {}, not {public_key}",
            account.public_key.into_address()
        ));
    }
    let rows = subtree_rows(leaves, 0);
    for (height, nodes) in rows.iter().take(ACCOUNT_SUBTREE_HEIGHT).enumerate() {
        let local = ((index >> height) & ((1 << (ACCOUNT_SUBTREE_HEIGHT - height)) - 1)) as usize;
        siblings[height] = nodes[local ^ 1];
    }
    let root = implied_root(account.hash(), index, &siblings);
    let ok = root == fp_of(ledger_hash);
    Ok(json!({
        "ledger_hash": ledger_hash.to_string(),
        "queries": queries,
        "accounts_in_subtree": accounts.len(),
        "path_verified": ok,
        "account": describe(&account),
    }))
}

/// `GET /light/account/{pk}?ledger=..&block=<tip>` and the checks a light
/// client makes: the answer is for the verified tip, its ledger hash is the
/// one the tip's protocol state names for that ledger, the account is the
/// one asked for and hashes to the leaf, and the path folds to the hash.
fn light_account(
    client: &reqwest::blocking::Client,
    base: &str,
    tip: (&MinaBlockHeaderStableV2, &StateHash),
    root: Option<(&MinaBlockHeaderStableV2, &StateHash)>,
    ledger: LightLedger,
    public_key: &str,
    bytes: &mut u64,
) -> Result<Value, String> {
    match light_account_at(client, base, tip.0, tip.1, ledger, public_key, bytes) {
        // The node holds the snarked ledger of its root, whose protocol
        // state the body hashes link to the verified tip: check that one.
        Err(LightError::Refused(refusal))
            if ledger == LightLedger::Snarked && refusal["code"] == "ledger_unavailable" =>
        {
            let (root, root_hash) = root.ok_or("no verified root to fall back to")?;
            let mut row =
                light_account_at(client, base, root, root_hash, ledger, public_key, bytes)
                    .map_err(|e| e.to_string())?;
            row["block"] = json!("root (linked to the verified tip)");
            row["tip_refusal"] = refusal;
            Ok(row)
        }
        other => other
            .map(|mut row| {
                row["block"] = json!("tip");
                row
            })
            .map_err(|e| e.to_string()),
    }
}

/// Why a light account check failed: refused by the node (with its JSON
/// answer), or anything else.
enum LightError {
    Refused(Value),
    Other(String),
}

impl std::fmt::Display for LightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(answer) => write!(f, "refused: {answer}"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

impl From<String> for LightError {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}

impl From<&str> for LightError {
    fn from(e: &str) -> Self {
        Self::Other(e.to_owned())
    }
}

/// [`light_account`] at one block: `block` is a header whose protocol state
/// the probe verified (the tip, or the root linked to it by body hashes).
fn light_account_at(
    client: &reqwest::blocking::Client,
    base: &str,
    tip: &MinaBlockHeaderStableV2,
    tip_hash: &StateHash,
    ledger: LightLedger,
    public_key: &str,
    bytes: &mut u64,
) -> Result<Value, LightError> {
    let url = format!(
        "{base}/light/account/{public_key}?ledger={}&block={tip_hash}",
        ledger.name()
    );
    let response = client
        .get(&url)
        .send()
        .and_then(|r| Ok((r.status(), r.bytes()?)))
        .map_err(|e| format!("GET {url}: {e}"))?;
    *bytes += response.1.len() as u64;
    let answer: Value = serde_json::from_slice(&response.1).map_err(|e| e.to_string())?;
    if !response.0.is_success() {
        return Err(LightError::Refused(answer));
    }
    let expected = ledger.hash_in(tip);
    let checks = [
        (
            "for the verified tip",
            answer["block"]["state_hash"].as_str() == Some(&tip_hash.to_string()),
        ),
        (
            "ledger hash from the verified state",
            answer["ledger_hash"].as_str() == Some(&expected.to_string()),
        ),
        (
            "ledger named as asked",
            answer["ledger"].as_str() == Some(ledger.name())
                && answer["ledger_hash_field"].as_str() == Some(ledger.field()),
        ),
    ];
    if let Some((what, _)) = checks.iter().find(|(_, ok)| !ok) {
        return Err(format!("answer is not {what}: {answer}").into());
    }
    let encoded = base64::engine::general_purpose::STANDARD
        .decode(answer["account"].as_str().ok_or("no account")?)
        .map_err(|e| e.to_string())?;
    let wire = MinaBaseAccountBinableArgStableV2::binprot_read(&mut &encoded[..])
        .map_err(|e| format!("decoding account: {e}"))?;
    let account = Account::try_from(&wire).map_err(|e| format!("{e:?}"))?;
    if account.public_key.into_address() != public_key {
        return Err(format!(
            "the answer is for {}, not {public_key}",
            account.public_key.into_address()
        )
        .into());
    }
    let leaf = account.hash();
    if Fp::from_str(answer["leaf_hash"].as_str().unwrap_or_default()).ok() != Some(leaf) {
        return Err("leaf_hash is not the account's hash".into());
    }
    let (siblings, path_index) = graphql_path(&answer["merkle_path"])?;
    let index = answer["index"].as_u64().ok_or("no index")?;
    if path_index != index || siblings.len() != LEDGER_DEPTH {
        return Err(format!(
            "path ({} elements, index {path_index}) does not match index {index}",
            siblings.len()
        )
        .into());
    }
    let verified = implied_root(leaf, index, &siblings) == fp_of(&expected);
    Ok(json!({
        "ledger_hash": expected.to_string(),
        "index": index,
        "verified": verified,
        "account": describe(&account),
    }))
}

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    let net = args.network.info();
    let mut report = json!({
        "network": format!("{:?}", args.network).to_lowercase(),
        "account": args.account,
        "started_at_unix": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
    });
    let mut phases = Phases::new();

    // Trust anchor: the verification key.
    let vk_path = args
        .vk
        .clone()
        .unwrap_or_else(|| format!("{}/../../data/vk/{}", env!("CARGO_MANIFEST_DIR"), net.vk));
    let vk_json = std::fs::read_to_string(&vk_path).map_err(|e| format!("{vk_path}: {e}"))?;
    let verifier = Verifier::from_vk_json(&vk_json).map_err(|e| e.to_string())?;
    verifier.warm_up();
    println!("verifier ready ({vk_path})");
    phases.mark("verifier_setup");

    let http_client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(args.timeout))
        .build()
        .map_err(|e| e.to_string())?;
    let mut graphql = (!args.no_graphql).then(|| Graphql {
        url: args
            .graphql
            .clone()
            .unwrap_or_else(|| net.graphql.to_string()),
        client: http_client.clone(),
        bytes: 0,
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| e.to_string())?;

    // 1. The tip, from a peer or from a node's HTTP API.
    let mut p2p = None;
    let (tip, root, body_hashes) = if let Some(base) = &args.http {
        let mut bytes = 0;
        let body = http_get(
            &http_client,
            &format!("{base}/light/best-tip?format=binprot"),
            &mut bytes,
        )?;
        let mut rest = &body[..];
        let header = MinaBlockHeaderStableV2::binprot_read(&mut rest)
            .map_err(|e| format!("decoding the tip header: {e}"))?;
        let root_state = MinaStateProtocolStateValueStableV2::binprot_read(&mut rest)
            .map_err(|e| format!("decoding the root state: {e}"))?;
        let body_hashes = List::<MinaBaseStateBodyHashStableV1>::binprot_read(&mut rest)
            .map_err(|e| format!("decoding the body hashes: {e}"))?;
        if !rest.is_empty() {
            return Err(format!("{} trailing bytes after the best tip", rest.len()));
        }
        report["tip_source"] = json!({
            "http": format!("{base}/light/best-tip"),
            "bytes": bytes,
            "binprot_bytes": {
                "tip_header": encoded_len(&header),
                "tip_protocol_state": encoded_len(&header.protocol_state),
                "tip_protocol_state_proof": encoded_len(&header.protocol_state_proof),
                "root_protocol_state": encoded_len(&root_state),
                "body_hashes": encoded_len(&body_hashes),
            },
        });
        println!(
            "best tip from {base}/light/best-tip: {bytes} B; sizes: {}",
            report["tip_source"]["binprot_bytes"]
        );
        // The root's protocol state alone: its proof is not served.
        let root = MinaBlockHeaderStableV2 {
            protocol_state: root_state,
            ..header.clone()
        };
        (header, Some(root), Some(body_hashes))
    } else {
        let peers: Vec<Multiaddr> = if args.peer.is_empty() {
            net.seeds.iter().map(|s| s.parse().unwrap()).collect()
        } else {
            args.peer
                .iter()
                .map(|s| s.parse().map_err(|e| format!("peer {s}: {e}")))
                .collect::<Result<_, _>>()?
        };
        let pnet_input = format!("/coda/0.0.1/{}", net.chain_id);
        let mut swarm = rt.block_on(async {
            mina_transport::swarm(
                Keypair::generate_ed25519(),
                pnet_input.as_bytes(),
                Vec::<Multiaddr>::new(),
                peers,
                rpc::RpcBehaviour::default(),
            )
        });
        let deadline = Duration::from_secs(args.timeout);
        let found = rt.block_on(async {
            tokio::time::timeout(deadline, async {
                loop {
                    let (peer, stream) = loop {
                        match swarm.next().await {
                            Some(SwarmEvent::Behaviour(opened)) => break opened,
                            Some(SwarmEvent::OutgoingConnectionError { error, .. }) => {
                                log::warn!("dial failed: {error}")
                            }
                            _ => {}
                        }
                    };
                    let mut conn = match rpc::RpcConn::open(peer, stream).await {
                        Ok(c) => c,
                        Err(e) => {
                            log::warn!("{peer}: {e}");
                            continue;
                        }
                    };
                    match rpc::drive(&mut swarm, conn.call::<GetBestTipV2>(&())).await {
                        Ok(Some(pcd)) => return (conn, pcd),
                        Ok(None) => log::warn!("{peer} has no best tip proof"),
                        Err(e) => log::warn!("{peer}: {e}"),
                    }
                }
            })
            .await
        });
        let (conn, pcd) = found.map_err(|_| "timed out waiting for a best tip".to_string())?;
        println!(
            "best tip from {}: {} B in, {} B out on the RPC stream",
            conn.peer, conn.bytes_in, conn.bytes_out
        );
        report["tip_source"] =
            json!({ "peer": conn.peer.to_string(), "rpc_bytes_in": conn.bytes_in });
        // What a header-only best-tip proof would need, against what was sent.
        report["tip_source"]["binprot_bytes"] = json!({
            "tip_block": encoded_len(&pcd.data),
            "tip_protocol_state": encoded_len(&pcd.data.header.protocol_state),
            "tip_protocol_state_proof": encoded_len(&pcd.data.header.protocol_state_proof),
            "root_block": encoded_len(&pcd.proof.1),
            "root_protocol_state": encoded_len(&pcd.proof.1.header.protocol_state),
            "body_hashes": encoded_len(&pcd.proof.0),
        });
        println!("  sizes: {}", report["tip_source"]["binprot_bytes"]);
        let (body_hashes, root) = pcd.proof;
        let tip = pcd.data.header;
        p2p = Some((swarm, conn));
        (tip, Some(root.header), Some(body_hashes))
    };
    phases.mark("fetch_tip");

    // 2. Verify: genesis pin, the tip's proof, and the root-to-tip hash chain.
    let tip_hash = tip.try_hash().map_err(|e| format!("{e:?}"))?;
    let genesis = tip.protocol_state.body.genesis_state_hash.to_string();
    if genesis != net.genesis_state_hash {
        return Err(format!(
            "tip carries genesis state hash {genesis}, expected {}",
            net.genesis_state_hash
        ));
    }
    if !verifier.verify_header(&tip) {
        return Err(format!("tip {tip_hash} proof does not verify"));
    }
    let cs = &tip.protocol_state.body.consensus_state;
    let height = cs.blockchain_length.as_u32();
    let ts_ms = tip
        .protocol_state
        .body
        .blockchain_state
        .timestamp
        .0
         .0
        .as_u64();
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    println!(
        "tip {tip_hash} height {height}: proof verified; {} s old; min window density {}",
        now_ms.saturating_sub(ts_ms) / 1000,
        cs.min_window_density.as_u32()
    );
    report["tip"] = json!({
        "state_hash": tip_hash.to_string(),
        "height": height,
        "global_slot_since_genesis": cs.global_slot_since_genesis.as_u32(),
        "age_s": now_ms.saturating_sub(ts_ms) / 1000,
        "min_window_density": cs.min_window_density.as_u32(),
        "snarked_ledger": snarked_ledger(&tip).to_string(),
        "staged_ledger": staged_ledger(&tip).to_string(),
        "staking_epoch_ledger": cs.staking_epoch_data.ledger.hash.to_string(),
        "next_epoch_ledger": cs.next_epoch_data.ledger.hash.to_string(),
        "proof_verified": true,
    });
    phases.mark("verify_tip");

    let account_pk = if args.account == "tip-winner" {
        cs.block_stake_winner.to_string()
    } else {
        args.account.clone()
    };
    report["account"] = json!(account_pk);

    if let (Some(root), Some(body_hashes)) = (&root, &body_hashes) {
        let root_hash = root.try_hash().map_err(|e| format!("{e:?}"))?;
        let mut h = root_hash.clone();
        for body in body_hashes.iter() {
            h = StateHash::try_from_hashes(&h, body).map_err(|e| format!("{e:?}"))?;
        }
        let root_is_genesis = root
            .protocol_state
            .body
            .consensus_state
            .blockchain_length
            .as_u32()
            <= 1;
        // A node serves its best chain as it is: shorter than k right
        // after it bootstraps. The hash chain still has to reach the tip.
        let full = body_hashes.len() == K || root_is_genesis;
        let chain_ok = h == tip_hash && (full || args.http.is_some());
        if !chain_ok {
            return Err(format!(
                "root-to-tip chain does not reach the tip ({} body hashes)",
                body_hashes.len()
            ));
        }
        // Over HTTP the root comes without its proof.
        let root_proof =
            (args.verify_root && args.http.is_none()).then(|| verifier.verify_header(root));
        println!(
            "root {root_hash} height {}: linked to the verified tip by {} body hashes{}",
            root.protocol_state
                .body
                .consensus_state
                .blockchain_length
                .as_u32(),
            body_hashes.len(),
            match root_proof {
                Some(true) => "; root proof verified too",
                Some(false) => "; ROOT PROOF FAILED",
                None => "",
            }
        );
        report["root"] = json!({
            "state_hash": root_hash.to_string(),
            "height": root.protocol_state.body.consensus_state.blockchain_length.as_u32(),
            "body_hashes": body_hashes.len(),
            "full_k": full,
            "snarked_ledger": snarked_ledger(root).to_string(),
            "proof_verified": root_proof,
        });
        phases.mark("verify_chain");
    }

    // 3. A Rust node's light endpoint: the account in each ledger the tip names.
    if let Some(base) = &args.http {
        let mut rows = Vec::new();
        for &ledger in &args.light_ledger {
            let start = Instant::now();
            let mut bytes = 0;
            let root_hash = root.as_ref().map(|root| root.try_hash()).transpose();
            let root_hash = root_hash.map_err(|e| format!("{e:?}"))?;
            let root_ref = root.as_ref().zip(root_hash.as_ref());
            let row = match light_account(
                &http_client,
                base,
                (&tip, &tip_hash),
                root_ref,
                ledger,
                &account_pk,
                &mut bytes,
            ) {
                Ok(row) => row,
                Err(e) => json!({ "verified": false, "error": e }),
            };
            let mut row = row;
            row["ledger"] = json!(ledger.name());
            row["bytes"] = json!(bytes);
            row["ms"] = json!(start.elapsed().as_millis() as u64);
            println!("light account, {} ledger: {row}", ledger.name());
            rows.push(row);
        }
        report["light_accounts"] = Value::Array(rows);
        phases.mark("light_accounts");
    }

    // 3a. GraphQL, right after the tip so its best tip is likely the same block:
    //     the index hint, the staged-ledger path, and the tip snarked-ledger path.
    let mut index = args.index;
    if let Some(gq) = graphql.as_mut() {
        let pk = &account_pk;
        let data = gq.query(&format!(
            "{{ bestChain(maxLength: 1) {{ stateHash }} account(publicKey: \"{pk}\") {{ index leafHash merklePath {{ left right }} }} }}"
        ))?;
        let their_tip = data["bestChain"][0]["stateHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let acc = &data["account"];
        if index.is_none() {
            index = acc["index"].as_u64();
        }
        let staged = if their_tip == tip_hash.to_string() {
            let (siblings, path_index) = graphql_path(&acc["merklePath"])?;
            let leaf = Fp::from_str(acc["leafHash"].as_str().ok_or("no leafHash")?)
                .map_err(|_| "bad leafHash")?;
            let ok = implied_root(leaf, path_index, &siblings) == fp_of(&staged_ledger(&tip));
            json!({ "same_tip": true, "path_verified": ok, "index": path_index, "leaf_hash": acc["leafHash"] })
        } else {
            json!({ "same_tip": false, "their_tip": their_tip })
        };
        println!("staged ledger (GraphQL account.merklePath): {staged}");
        report["staged_graphql"] = staged;

        let membership = gq.query(&format!(
            "{{ encodedSnarkedLedgerAccountMembership(accountInfos: [{{publicKey: \"{pk}\"}}], stateHash: \"{tip_hash}\") {{ account merklePath {{ left right }} }} }}"
        ));
        match membership {
            Err(e) => {
                println!("tip snarked ledger (GraphQL encodedSnarkedLedgerAccountMembership): {e}");
                report["tip_snarked_graphql"] = json!({ "error": e });
            }
            Ok(data) => {
                let m = &data["encodedSnarkedLedgerAccountMembership"][0];
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(m["account"].as_str().ok_or("no account")?)
                    .map_err(|e| e.to_string())?;
                let wire = MinaBaseAccountBinableArgStableV2::binprot_read(&mut &bytes[..])
                    .map_err(|e| format!("decoding account: {e}"))?;
                let account = Account::try_from(&wire).map_err(|e| format!("{e:?}"))?;
                let (siblings, path_index) = graphql_path(&m["merklePath"])?;
                let ok = account.public_key.into_address() == *pk
                    && implied_root(account.hash(), path_index, &siblings)
                        == fp_of(&snarked_ledger(&tip));
                let snarked = json!({
                    "ledger_hash": snarked_ledger(&tip).to_string(),
                    "index": path_index,
                    "path_verified": ok,
                    "account": describe(&account),
                });
                println!(
                    "tip snarked ledger (GraphQL encodedSnarkedLedgerAccountMembership): {snarked}"
                );
                report["tip_snarked_graphql"] = snarked;
                if let Some(staged_leaf) = report["staged_graphql"]["leaf_hash"].as_str() {
                    let same = Fp::from_str(staged_leaf).ok() == Some(account.hash());
                    report["staged_graphql"]["leaf_equals_snarked_account"] = json!(same);
                }
            }
        }
        report["graphql_bytes"] = json!(gq.bytes);
        phases.mark("graphql_accounts");
    }

    // 3b. The peer: the account and its path against a ledger from the verified state.
    if let Some((mut swarm, mut conn)) = p2p.take() {
        let index = index.ok_or("no account index: pass --index")?;
        let ledger_hash = match args.ledger {
            LedgerChoice::Root => snarked_ledger(root.as_ref().expect("p2p mode has a root")),
            LedgerChoice::Staking => cs.staking_epoch_data.ledger.hash.clone(),
            LedgerChoice::Next => cs.next_epoch_data.ledger.hash.clone(),
            LedgerChoice::TipSnarked => snarked_ledger(&tip),
        };
        let before = (conn.bytes_in, conn.bytes_out, conn.calls);
        let result = rt.block_on(p2p_account(
            &mut swarm,
            &mut conn,
            &ledger_hash,
            index,
            &account_pk,
            args.subtree_depth,
        ));
        let mut row = match result {
            Ok(v) => v,
            Err(e) => json!({ "ledger_hash": ledger_hash.to_string(), "error": e }),
        };
        row["ledger"] = json!(format!("{:?}", args.ledger));
        row["rpc_bytes_in"] = json!(conn.bytes_in - before.0);
        row["rpc_bytes_out"] = json!(conn.bytes_out - before.1);
        println!(
            "p2p {:?} ledger (answer_sync_ledger_query): {row}",
            args.ledger
        );
        report["p2p_account"] = row;
        phases.mark("p2p_account");
        drop(conn);
        drop(swarm);
    }

    report["phases"] = Value::Array(phases.rows.clone());
    report["total_ms"] = json!(phases.start.elapsed().as_millis() as u64);
    report["peak_rss_kib"] = json!(vm_hwm_kib());
    println!(
        "total {} ms, peak RSS {} MiB",
        report["total_ms"],
        vm_hwm_kib() / 1024
    );
    if let Some(path) = &args.report {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer OCaml gave (Auro's devnet GraphQL, 2026-10-24 20:59 UTC, LN1)
    /// folds, through the probe's own parsing, to one of the snarked ledger
    /// hashes OCaml's `bestChain(maxLength: 290)` reported minutes earlier.
    #[test]
    fn ocaml_encoded_membership_folds_to_an_ocaml_snarked_ledger_hash() {
        // Every distinct `snarkedLedgerHash` of that bestChain answer.
        const OCAML_SNARKED: &[&str] = &[
            "jxezNWDF3tBPRmH5xCguyZdFDTUYbPtZBFBrgNxkQiQNk4fbaEx",
            "jxbWwwCDTu9mZ1vf6EmYLySXtLqLvbWQcMXXoeJDUH4VercS1sX",
            "jxccR1Jm98Dmdqgp3n72gY1nfxETGuo2Jc26z3WgPUCa6eZJ1kM",
            "jy1huPEiJtY92tpvwbmedExaNzcyrKeq5kWGUyHxqopAecCngxh",
            "jwvNvquknUF18DdXW2yS1qvCPCvPr2Vi1saqzBghrL18X7mX1BC",
            "jwMrUpDrFuTXHS1jbRf1JMp46PvabNpHzarp34ef6Y5xHC8kAVT",
        ];
        let answer: Value = serde_json::from_str(include_str!(
            "../testdata/ocaml-encoded-membership-devnet.json"
        ))
        .unwrap();
        let m = &answer["data"]["encodedSnarkedLedgerAccountMembership"][0];
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(m["account"].as_str().unwrap())
            .unwrap();
        let wire = MinaBaseAccountBinableArgStableV2::binprot_read(&mut &bytes[..]).unwrap();
        let account = Account::try_from(&wire).unwrap();
        assert_eq!(
            account.public_key.into_address(),
            "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
        );
        let (siblings, index) = graphql_path(&m["merklePath"]).unwrap();
        assert_eq!(siblings.len(), LEDGER_DEPTH);
        let root = LedgerHash::from_fp(implied_root(account.hash(), index, &siblings));
        assert!(
            OCAML_SNARKED.contains(&root.to_string().as_str()),
            "{root} is none of OCaml's snarked ledger hashes"
        );
    }
}
