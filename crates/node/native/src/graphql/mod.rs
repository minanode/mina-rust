use crate::input_limits::{self as limits, bounded};
use account::{create_account_loader, AccountLoader, GraphQLAccount};
use block::{GraphQLBlock, GraphQLSnarkJob, GraphQLUserCommands};
use juniper::{graphql_value, FieldError, GraphQLEnum, ID};
use ledger::{Account, AccountId};
use mina_core::{
    block::AppliedBlock, consensus::ConsensusConstants, constants::constraint_constants,
    NetworkConfig,
};
use mina_node::{
    account::AccountPublicKey,
    ledger::read::LedgerStatus,
    rpc::{
        AccountQuery, DelegatorsQuery, GetBlockQuery, PooledCommandsQuery, RpcBestChainResponse,
        RpcGenesisBlockResponse, RpcGetBlockResponse, RpcLedgerAccountDelegatorsGetResponse,
        RpcLedgerStatusGetResponse, RpcMetricsGetResponse, RpcNodeStatus,
        RpcPooledUserCommandsResponse, RpcPooledZkappCommandsResponse, RpcRequest,
        RpcSnarkPoolCompletedJobsResponse, RpcSnarkPoolPendingJobsGetResponse, RpcSnarkerConfig,
        RpcStatusGetResponse, RpcTransactionInjectResponse, RpcTransactionStatusGetResponse,
    },
    BuildEnv,
};
use mina_node_common::rpc::RpcSender;
use mina_p2p_messages::v2::{
    conv, LedgerHash, MinaBaseSignedCommandStableV2, MinaBaseUserCommandStableV2,
    MinaBaseZkappCommandTStableV1WireStableV1, TokenIdKeyHash, TransactionHash,
};
use mina_signer::CompressedPubKey;
use o1_utils::field_helpers::FieldHelpersError;
use scalars::{PublicKeyScalar, TokenIdScalar};
use snark::{GraphQLPendingSnarkWork, GraphQLSnarkWorker};
use std::str::FromStr;
use tokio::sync::OnceCell;
use transaction::GraphQLTransactionStatus;
use zkapp::GraphQLZkapp;

pub mod account;
pub mod block;
pub mod constants;
pub mod membership;
pub(crate) mod ocaml_errors;
pub mod scalars;
pub mod snark;
pub mod transaction;
pub mod user_command;
pub mod verification;
mod work_id;
pub mod zkapp;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod send_tests;

/// Base58 encoded public key
pub type GraphQLPublicKey = String;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Conversion error: {0}")]
    Conversion(ConversionError),
    #[error("State machine empty response")]
    StateMachineEmptyResponse,
    #[error("Custom: {0}")]
    Custom(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error(transparent)]
    InputTooLong(#[from] crate::input_limits::InputTooLong),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Conversion(#[from] mina_p2p_messages::v2::conv::Error),
    #[error("Wrong variant")]
    WrongVariant,
    #[error("SerdeJson: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("Base58Check: {0}")]
    Base58Check(#[from] mina_p2p_messages::b58::FromBase58CheckError),
    #[error("Base58 error: {0}")]
    Base58(#[from] bs58::decode::Error),
    #[error(transparent)]
    InvalidDecimalNumber(#[from] mina_p2p_messages::bigint::InvalidDecimalNumber),
    #[error("Invalid bigint")]
    InvalidBigInt,
    #[error("Invalid hex")]
    InvalidHex,
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)]
    EnumParse(#[from] strum::ParseError),
    #[error(transparent)]
    TryFromInt(#[from] std::num::TryFromIntError),
    #[error("Missing field: {0}")]
    MissingField(String),
    #[error("Invalid length")]
    InvalidLength,
    #[error("Custom: {0}")]
    Custom(String),
    #[error(transparent)]
    FieldHelpers(#[from] FieldHelpersError),
    #[error("Failed to convert integer to i32")]
    Integer,
}

impl From<ConversionError> for Error {
    fn from(value: ConversionError) -> Self {
        Error::Conversion(value)
    }
}

/// Context for the GraphQL API
///
/// This is used to share state between the GraphQL queries and mutations.
///
/// The caching used here is only valid for the lifetime of the context i.e. for
/// one request which is the goal as we can have multiple sources for one
/// request.
/// This optimizes the number of request to the state machine
pub struct Context {
    rpc_sender: RpcSender,
    /// Whether the mutations that submit transactions are allowed. Off by
    /// default; `--enable-tx-submission` turns it on.
    tx_submission: bool,
    public_graphql: bool,
    account_loader: AccountLoader,
    light_path_loader: membership::LightPathLoader,
    // Caches
    statemachine_status_cache: OnceCell<Option<RpcNodeStatus>>,
    best_tip_cache: OnceCell<Option<AppliedBlock>>,
    ledger_status_cache: OnceCell<Option<LedgerStatus>>,
}

impl juniper::Context for Context {}

impl Context {
    pub fn new(rpc_sender: RpcSender) -> Self {
        Self {
            rpc_sender: rpc_sender.clone(),
            tx_submission: false,
            public_graphql: false,
            statemachine_status_cache: OnceCell::new(),
            best_tip_cache: OnceCell::new(),
            ledger_status_cache: OnceCell::new(),
            account_loader: create_account_loader(rpc_sender.clone()),
            light_path_loader: membership::create_light_path_loader(rpc_sender.clone()),
        }
    }

    /// Allows or refuses the mutations that submit transactions.
    pub fn with_tx_submission(mut self, enabled: bool) -> Self {
        self.tx_submission = enabled;
        self
    }

    /// Applies the public router's resolver work limits.
    pub fn with_public_graphql(mut self, public: bool) -> Self {
        self.public_graphql = public;
        self
    }

    /// An error for a submission when submission is off on this node.
    fn ensure_tx_submission(&self) -> juniper::FieldResult<()> {
        if self.tx_submission {
            Ok(())
        } else {
            Err(FieldError::new(
                crate::http_server::TX_SUBMISSION_DISABLED,
                graphql_value!(null),
            ))
        }
    }

    pub(crate) async fn get_or_fetch_status(&self) -> RpcStatusGetResponse {
        self.statemachine_status_cache
            .get_or_init(|| async {
                self.rpc_sender
                    .oneshot_request(RpcRequest::StatusGet)
                    .await
                    .flatten()
            })
            .await
            .clone()
    }

    pub(crate) async fn get_or_fetch_best_tip(&self) -> Option<AppliedBlock> {
        self.best_tip_cache
            .get_or_init(|| async {
                self.rpc_sender
                    .oneshot_request(RpcRequest::BestChain(1))
                    .await
                    .and_then(|blocks: RpcBestChainResponse| blocks.first().cloned())
            })
            .await
            .clone()
    }

    pub(crate) async fn get_or_fetch_ledger_status(
        &self,
        ledger_hash: &LedgerHash,
    ) -> RpcLedgerStatusGetResponse {
        self.ledger_status_cache
            .get_or_init(|| async {
                self.rpc_sender
                    .oneshot_request(RpcRequest::LedgerStatusGet(ledger_hash.clone()))
                    .await
                    .flatten()
            })
            .await
            .clone()
    }

    pub(crate) async fn load_account(&self, account_id: AccountId) -> Option<GraphQLAccount> {
        self.account_loader.try_load(account_id).await.ok()?.ok()
    }

    pub async fn fetch_delegators(
        &self,
        query: DelegatorsQuery,
        account_id: AccountId,
    ) -> RpcLedgerAccountDelegatorsGetResponse {
        self.rpc_sender
            .oneshot_request(RpcRequest::LedgerAccountDelegatorsGet(query, account_id))
            .await
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, GraphQLEnum)]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum SyncStatus {
    CONNECTING,
    LISTENING,
    OFFLINE,
    BOOTSTRAP,
    SYNCED,
    CATCHUP,
}

#[derive(Clone, Copy, Debug)]
pub struct Query;

/// GraphQL Query endpoints exposed by the Mina node
///
/// # Available Queries:
///
/// ## Account Management
/// - `account` - Retrieve account information for a public key
/// - `current_snark_worker` - Get information about the current SNARK worker
///
/// ## Blockchain State
/// - `sync_status` - Get the synchronization status of the node
/// - `best_chain` - Retrieve the best chain of blocks
/// - `block` - Get a specific block by hash or height
/// - `genesis_block` - Retrieve the genesis block
/// - `genesis_constants` - Get genesis configuration constants
/// - `daemon_status` - Get the daemon status information
///
/// ## Transaction Pool
/// - `pooled_user_commands` - Query pending user commands in the transaction
///   pool
/// - `pooled_zkapp_commands` - Query pending zkApp commands in the transaction
///   pool
/// - `transaction_status` - Check the status of a transaction
///
/// ## SNARK Pool
/// - `snark_pool` - Get completed SNARK jobs
/// - `pending_snark_work` - Get pending SNARK work items
///
/// ## Network Information
/// - `network_id` - Get the chain-agnostic network identifier
/// - `version` - Get the node version (git commit hash)
#[juniper::graphql_object(context = Context)]
impl Query {
    /// Retrieve account information for a given public key
    ///
    /// # Arguments
    /// - `public_key`: Base58-encoded public key
    /// - `token`: Optional token ID (defaults to MINA token if not provided)
    ///
    /// # Returns
    /// Account information including balance, nonce, delegate, and other fields
    ///
    /// Like OCaml, this is null when the account does not exist, and the
    /// token defaults to MINA. It looks the one account up by its id.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L1940-L1960>
    /// Bounded receipts produced by this process; no archive or peer claims.
    async fn verification(
        context: &Context,
    ) -> Result<verification::LocalVerificationStats, FieldError> {
        let snapshot = context
            .rpc_sender
            .oneshot_request::<mina_node::rpc::RpcVerificationStatsGetResponse>(
                RpcRequest::VerificationStatsGet(
                    mina_node::verification::VerificationQuery::summary(),
                ),
            )
            .await
            .ok_or_else(|| {
                FieldError::new("Verification telemetry unavailable", graphql_value!(null))
            })?;
        Ok(snapshot.into())
    }

    /// This node's verification receipts of up to 20 blocks, by state hash,
    /// in the order given; a block it keeps no receipt for is left out. Not
    /// in OCaml's schema. A fixed public projection of the receipts that
    /// `/stats/verification` serves its operator, allowed with
    /// `--public-graphql`.
    async fn verification_receipts(
        state_hashes: Vec<String>,
        context: &Context,
    ) -> juniper::FieldResult<Vec<verification::PublicVerificationReceipt>> {
        verification::public_receipts(context, state_hashes).await
    }

    async fn account(
        public_key: PublicKeyScalar,
        token: Option<TokenIdScalar>,
        context: &Context,
    ) -> juniper::FieldResult<Option<account::GraphQLAccount>> {
        let public_key = public_key_argument(&public_key, "account")?;
        let token_id = match token {
            None => TokenIdKeyHash::default(),
            Some(token) => token_id_argument(&token, "token", "account")?,
        };
        let accounts: Vec<Account> = context
            .rpc_sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(
                AccountQuery::PubKeyWithTokenId(public_key, token_id),
            ))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(accounts
            .into_iter()
            .next()
            .map(TryInto::try_into)
            .transpose()?)
    }

    /// obtain a membership proof for an account in the snarked ledger along
    /// with the accounts full information encoded as base64 binable type
    ///
    /// The snarked ledger of the best-chain block `stateHash`. At most
    /// [`membership::MAX_MEMBERSHIP_ACCOUNTS`] (32) accounts per field, read
    /// in one ledger call under the light request limit.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2481-L2500>
    async fn encoded_snarked_ledger_account_membership(
        account_infos: Vec<membership::AccountInput>,
        state_hash: String,
        context: &Context,
    ) -> juniper::FieldResult<Vec<membership::GraphQLEncodedAccount>> {
        membership::encoded_snarked_ledger_account_membership(context, account_infos, state_hash)
            .await
    }

    /// All the accounts of a public key, one per token
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L1962-L1984>
    async fn accounts(
        public_key: PublicKeyScalar,
        context: &Context,
    ) -> juniper::FieldResult<Vec<account::GraphQLAccount>> {
        let public_key = public_key_argument(&public_key, "accounts")?;
        let accounts: Vec<Account> = context
            .rpc_sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(
                AccountQuery::SinglePublicKey(public_key),
            ))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(accounts
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Find the account that owns a given token
    ///
    /// Every account owns the token its id derives; the default token has
    /// no owner, which needs no ledger lookup.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2020-L2036>
    async fn token_owner(
        token_id: TokenIdScalar,
        context: &Context,
    ) -> juniper::FieldResult<Option<account::GraphQLAccount>> {
        let token_id = token_id_argument(&token_id, "tokenId", "tokenOwner")?;
        if token_id == TokenIdKeyHash::default() {
            return Ok(None);
        }
        let accounts: Vec<Account> = context
            .rpc_sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(AccountQuery::TokenOwner(
                token_id,
            )))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(accounts
            .into_iter()
            .next()
            .map(TryInto::try_into)
            .transpose()?)
    }

    /// Accounts for which the daemon tracks the private key. This node keeps
    /// no wallet keys, so the list is empty, as OCaml's is without any.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L1917-L1922>
    async fn tracked_accounts(
        _context: &Context,
    ) -> juniper::FieldResult<Vec<account::GraphQLAccount>> {
        Ok(vec![])
    }

    /// Get the current synchronization status of the node
    ///
    /// # Returns
    /// One of: CONNECTING, LISTENING, OFFLINE, BOOTSTRAP, SYNCED, CATCHUP
    async fn sync_status(context: &Context) -> juniper::FieldResult<SyncStatus> {
        fetch_sync_status(context).await
    }

    /// Retrieve a list of blocks from transition frontier's root to the
    /// current best tip
    ///
    /// # Arguments
    /// - `max_length`: The maximum number of blocks to return. If there are
    ///   more blocks in the transition frontier from root to tip, the
    ///   maxLength blocks closest to the best tip are returned. Without it,
    ///   the whole best chain, as OCaml.
    ///
    /// # Returns
    /// The blocks, oldest first
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2157-L2182>
    async fn best_chain(
        max_length: Option<i32>,
        context: &Context,
    ) -> juniper::FieldResult<Vec<GraphQLBlock>> {
        let max_length = max_length.map_or(u32::MAX, |n| n.max(0) as u32);
        let best_chain: Vec<AppliedBlock> = context
            .rpc_sender
            .oneshot_request(RpcRequest::BestChain(max_length))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(best_chain
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Get daemon status information including chain ID and configuration
    ///
    /// # Returns
    /// Daemon status with chain ID, configuration, and other metadata
    async fn daemon_status(
        _context: &Context,
    ) -> juniper::FieldResult<constants::GraphQLDaemonStatus> {
        Ok(constants::GraphQLDaemonStatus)
    }

    /// Retrieve genesis configuration constants
    ///
    /// # Returns
    /// Genesis constants including protocol parameters and constraints
    async fn genesis_constants(
        context: &Context,
    ) -> juniper::FieldResult<constants::GraphQLGenesisConstants> {
        let consensus_constants: ConsensusConstants = context
            .rpc_sender
            .oneshot_request(RpcRequest::ConsensusConstantsGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;
        let constraint_constants = constraint_constants();

        Ok(constants::GraphQLGenesisConstants::try_new(
            constraint_constants.clone(),
            consensus_constants,
        )?)
    }

    /// Check the status of a transaction
    ///
    /// # Arguments
    /// - `payment`: Base64-encoded signed command (mutually exclusive with
    ///   zkapp_transaction)
    /// - `zkapp_transaction`: Base64-encoded zkApp command (mutually exclusive with payment)
    ///
    /// # Returns
    /// Transaction status (PENDING, INCLUDED, or UNKNOWN)
    ///
    /// The errors are OCaml's: an id that does not decode is
    /// `Invalid transaction provided (<why>)`, with ocaml-base64's reason
    /// when it is not Base64.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2038-L2081>
    async fn transaction_status(
        payment: Option<ID>,
        zkapp_transaction: Option<ID>,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLTransactionStatus> {
        fn decode<T>(
            id: &str,
            max_len: usize,
            from_base64: impl Fn(&str) -> Result<T, conv::Error>,
        ) -> Result<T, FieldError> {
            let why = match ocaml_errors::base64_error(id) {
                Some(why) => why.to_owned(),
                None => match bounded(id, max_len).map_err(ConversionError::from) {
                    Ok(id) => match from_base64(id) {
                        Ok(tx) => return Ok(tx),
                        Err(e) => e.to_string(),
                    },
                    Err(e) => e.to_string(),
                },
            };
            Err(FieldError::new(
                format!("Invalid transaction provided ({why})"),
                graphql_value!(null),
            ))
        }

        let tx = match (payment, zkapp_transaction) {
            (Some(payment), None) => MinaBaseUserCommandStableV2::SignedCommand(decode(
                &payment,
                limits::SIGNED_COMMAND,
                MinaBaseSignedCommandStableV2::from_base64,
            )?),
            (None, Some(zkapp_transaction)) => MinaBaseUserCommandStableV2::ZkappCommand(decode(
                &zkapp_transaction,
                limits::OPAQUE_BASE64,
                MinaBaseZkappCommandTStableV1WireStableV1::from_base64,
            )?),
            _ => {
                return Err(FieldError::new(
                    "Invalid query: Specify either a payment ID or a zkApp transaction ID",
                    graphql_value!(null),
                ));
            }
        };
        let res: RpcTransactionStatusGetResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::TransactionStatusGet(tx))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(GraphQLTransactionStatus::from(res))
    }

    /// Retrieve a block with the given state hash or height from the transition frontier
    ///
    /// # Arguments
    /// - `height`: Block height (mutually exclusive with state_hash)
    /// - `state_hash`: Block state hash (mutually exclusive with height)
    ///
    /// # Returns
    /// Block data including transactions, SNARK jobs, and metadata
    ///
    /// The errors are OCaml's. A height is taken as OCaml's
    /// `Unsigned.UInt32.of_int` takes it, so a negative one wraps.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2282-L2328>
    async fn block(
        height: Option<i32>,
        state_hash: Option<String>,
        context: &Context,
    ) -> juniper::FieldResult<GraphQLBlock> {
        let query = match (height, state_hash) {
            (Some(height), None) => GetBlockQuery::Height(height as u32),
            (None, Some(state_hash)) => {
                // Bounded first: Base58 decoding is quadratic in the length.
                let state_hash = bounded(&state_hash, limits::STATE_HASH)?;
                if let Some(error) = ocaml_errors::base58_check_error(
                    state_hash,
                    mina_p2p_messages::b58version::STATE_HASH,
                    "State hash",
                ) {
                    return Err(FieldError::new(error, graphql_value!(null)));
                }
                GetBlockQuery::Hash(state_hash.parse()?)
            }
            _ => {
                return Err(FieldError::new(
                    "Must provide exactly one of state hash, height",
                    graphql_value!(null),
                ));
            }
        };

        let res: Option<RpcGetBlockResponse> = context
            .rpc_sender
            .oneshot_request(RpcRequest::GetBlock(query.clone()))
            .await;

        match res {
            None => Err(Error::Custom("response channel dropped".to_owned()).into()),
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2672-L2702>
            Some(None) => Err(FieldError::new(
                match query {
                    GetBlockQuery::Hash(hash) => {
                        format!("Block with state hash {hash} not found in transition frontier")
                    }
                    GetBlockQuery::Height(height) => {
                        format!("Could not find block in transition frontier with height {height}")
                    }
                },
                graphql_value!(null),
            )),
            Some(Some(block)) => Ok(GraphQLBlock::try_from(block)?),
        }
    }

    /// Retrieve all the scheduled user commands for a specified sender that
    /// the current daemon sees in its transaction pool. All scheduled
    /// commands are queried if no sender is specified
    ///
    /// Arguments:
    /// - `public_key`: base58 encoded [`AccountPublicKey`]
    /// - `hashes`: list of base58 encoded [`TransactionHash`]es
    /// - `ids`: list of base64 encoded [`MinaBaseZkappCommandTStableV1WireStableV1`]
    async fn pooled_user_commands(
        &self,
        public_key: Option<PublicKeyScalar>,
        hashes: Option<Vec<String>>,
        ids: Option<Vec<ID>>,
        context: &Context,
    ) -> juniper::FieldResult<Vec<GraphQLUserCommands>> {
        let query = parse_pooled_commands_query(
            public_key
                .map(|key| public_key_argument(&key, "pooledUserCommands"))
                .transpose()?,
            hashes,
            ids,
            limits::SIGNED_COMMAND,
            MinaBaseSignedCommandStableV2::from_base64,
        )?;

        let res: RpcPooledUserCommandsResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::PooledUserCommands(query))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(res
            .into_iter()
            .map(GraphQLUserCommands::try_from)
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Retrieve all the scheduled zkApp commands for a specified sender that
    ///  the current daemon sees in its transaction pool. All scheduled
    ///  commands are queried if no sender is specified
    ///
    /// Arguments:
    /// - `public_key`: base58 encoded [`AccountPublicKey`]
    /// - `hashes`: list of base58 encoded [`TransactionHash`]es
    /// - `ids`: list of base64 encoded [`MinaBaseZkappCommandTStableV1WireStableV1`]
    async fn pooled_zkapp_commands(
        public_key: Option<PublicKeyScalar>,
        hashes: Option<Vec<String>>,
        ids: Option<Vec<ID>>,
        context: &Context,
    ) -> juniper::FieldResult<Vec<GraphQLZkapp>> {
        let query = parse_pooled_commands_query(
            public_key
                .map(|key| public_key_argument(&key, "pooledZkappCommands"))
                .transpose()?,
            hashes,
            ids,
            limits::OPAQUE_BASE64,
            MinaBaseZkappCommandTStableV1WireStableV1::from_base64,
        )?;

        let res: RpcPooledZkappCommandsResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::PooledZkappCommands(query))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(res
            .into_iter()
            .map(GraphQLZkapp::try_from)
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Retrieve the genesis block
    ///
    /// # Returns
    /// The genesis block data. Its `stagedLedgerProofEmitted` comes from the
    /// transition frontier, as for every block in OCaml, so it is null when
    /// the genesis block is not in the frontier (a hard fork genesis).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L984-L1001>
    async fn genesis_block(context: &Context) -> juniper::FieldResult<GraphQLBlock> {
        let block = context
            .rpc_sender
            .oneshot_request::<RpcGenesisBlockResponse>(RpcRequest::GenesisBlockGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?
            .ok_or(Error::StateMachineEmptyResponse)?;
        let in_frontier: RpcGetBlockResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::GetBlock(GetBlockQuery::Hash(
                block.hash.clone(),
            )))
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(GraphQLBlock::genesis(
            block,
            in_frontier.map(|b| b.just_emitted_a_proof),
        )?)
    }

    /// Get completed SNARK jobs from the SNARK pool
    ///
    /// # Returns
    /// List of completed SNARK jobs with proofs and fees
    async fn snark_pool(context: &Context) -> juniper::FieldResult<Vec<GraphQLSnarkJob>> {
        let jobs: RpcSnarkPoolCompletedJobsResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::SnarkPoolCompletedJobsGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(jobs.iter().map(GraphQLSnarkJob::from).collect())
    }

    /// Get pending SNARK work items that need to be completed
    ///
    /// # Returns
    /// List of pending SNARK work items with job specifications
    async fn pending_snark_work(
        context: &Context,
    ) -> juniper::FieldResult<Vec<GraphQLPendingSnarkWork>> {
        let jobs: RpcSnarkPoolPendingJobsGetResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::SnarkPoolPendingJobsGet)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        Ok(jobs
            .into_iter()
            .map(GraphQLPendingSnarkWork::try_from)
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// The peers this node is connected to
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2337-L2344>
    async fn get_peers(
        context: &Context,
    ) -> juniper::FieldResult<Vec<constants::GraphQLRpcPeerInfo>> {
        constants::fetch_connected_peers(context).await
    }

    /// The peers the daemon first used to connect to the network. Not exposed
    /// by this node's state machine yet, so the list is empty.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2328-L2335>
    async fn initial_peers(_context: &Context) -> juniper::FieldResult<Vec<String>> {
        Ok(vec![])
    }

    /// The chain-agnostic identifier of the network
    ///
    /// # Returns
    /// Network identifier in the format "mina:<network_name>"
    #[graphql(name = "networkID")]
    async fn network_id(_context: &Context) -> juniper::FieldResult<String> {
        let res = format!("mina:{}", NetworkConfig::global().name);
        Ok(res)
    }

    /// The version of the node (git commit hash)
    ///
    /// # Returns
    /// Git commit hash of the current node build
    async fn version(_context: &Context) -> juniper::FieldResult<String> {
        let res = BuildEnv::get().git.commit_hash;
        Ok(res)
    }

    /// Get information about the current SNARK worker if configured
    ///
    /// # Returns
    /// SNARK worker configuration including public key, account, and fee
    async fn current_snark_worker(
        &self,
        context: &Context,
    ) -> juniper::FieldResult<Option<GraphQLSnarkWorker>> {
        let config: Option<RpcSnarkerConfig> = context
            .rpc_sender
            .oneshot_request(RpcRequest::SnarkerConfig)
            .await
            .ok_or(Error::StateMachineEmptyResponse)?;

        let Some(config) = config else {
            return Ok(None);
        };

        let account = context
            .load_account(AccountId {
                public_key: CompressedPubKey::try_from(&config.public_key)?,
                token_id: ledger::TokenId::default(),
            })
            .await;

        Ok(Some(GraphQLSnarkWorker {
            key: config.public_key.to_string(),
            account,
            fee: config.fee.to_string(),
        }))
    }
}

/// The sync status the `syncStatus` query and `daemonStatus` report: the
/// phase `/status` and `/readyz` go by (F-dash3), `CONNECTING` and
/// `LISTENING` until the node has heard a best tip from a peer, as OCaml's.
pub(crate) async fn fetch_sync_status(context: &Context) -> juniper::FieldResult<SyncStatus> {
    let snapshot: RpcMetricsGetResponse = context
        .rpc_sender
        .oneshot_request(RpcRequest::MetricsGet)
        .await
        .ok_or(Error::StateMachineEmptyResponse)?;
    Ok(sync_status(&snapshot.sync_phase))
}

fn sync_status(phase: &str) -> SyncStatus {
    match phase {
        "Connecting" => SyncStatus::CONNECTING,
        "Bootstrap" => SyncStatus::BOOTSTRAP,
        "Catchup" => SyncStatus::CATCHUP,
        "Synced" => SyncStatus::SYNCED,
        _ => SyncStatus::LISTENING,
    }
}

async fn inject_tx<R>(
    cmd: MinaBaseUserCommandStableV2,
    context: &Context,
) -> juniper::FieldResult<R>
where
    R: TryFrom<MinaBaseUserCommandStableV2>,
{
    let res: RpcTransactionInjectResponse = context
        .rpc_sender
        .oneshot_request(RpcRequest::TransactionInject(vec![cmd]))
        .await
        .ok_or(Error::StateMachineEmptyResponse)?;

    match res {
        RpcTransactionInjectResponse::Success(res) => {
            let cmd: MinaBaseUserCommandStableV2 = match res.first().cloned() {
                Some(cmd) => cmd.into(),
                _ => unreachable!(),
            };
            cmd.try_into().map_err(|_| {
                FieldError::new(
                    "Failed to convert transaction to the required type".to_string(),
                    graphql_value!(null),
                )
            })
        }
        RpcTransactionInjectResponse::Rejected(rejected) => {
            let error_list = rejected
                .into_iter()
                .map(|(_, err)| graphql_value!({ "message": err.to_string() }))
                .collect::<Vec<_>>();

            Err(FieldError::new(
                "Transaction rejected",
                graphql_value!(juniper::Value::List(error_list)),
            ))
        }
        RpcTransactionInjectResponse::Failure(failure) => {
            let error_list = failure
                .into_iter()
                .map(|err| graphql_value!({ "message": err.to_string() }))
                .collect::<Vec<_>>();

            Err(FieldError::new(
                "Transaction failed",
                graphql_value!(juniper::Value::List(error_list)),
            ))
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mutation;

/// GraphQL Mutation endpoints exposed by the Mina node
///
/// # Available Mutations:
///
/// ## Transaction Submission
/// - `send_zkapp` - Submit a zkApp transaction to the network
/// - `send_payment` - Send a payment transaction
/// - `send_delegation` - Send a delegation transaction
#[juniper::graphql_object(context = Context)]
impl Mutation {
    /// Submit a zkApp transaction to the network
    ///
    /// # Arguments
    /// - `input`: zkApp command with account updates and fee payer information
    ///
    /// # Returns
    /// Transaction response with hash and zkApp command details
    async fn send_zkapp(
        input: zkapp::SendZkappInput,
        context: &Context,
    ) -> juniper::FieldResult<zkapp::GraphQLSendZkappResponse> {
        context.ensure_tx_submission()?;
        inject_tx(input.try_into()?, context).await
    }

    /// Send a payment transaction
    ///
    /// # Arguments
    /// - `input`: Payment details including sender, receiver, amount, fee, and memo
    /// - `signature`: Transaction signature
    ///
    /// # Returns
    /// Payment response with transaction hash and details
    async fn send_payment(
        input: user_command::InputGraphQLPayment,
        signature: Option<user_command::UserCommandSignature>,
        context: &Context,
    ) -> juniper::FieldResult<user_command::GraphQLSendPaymentResponse> {
        context.ensure_tx_submission()?;
        let signature = signature.ok_or_else(|| field_error(NO_UNLOCKED_KEY))?;
        let (common, body) = input.parse().map_err(field_error)?;
        let receiver = match &body {
            ledger::scan_state::transaction_logic::signed_command::Body::Payment(payment) => {
                Some((payment.receiver_pk.clone(), payment.amount))
            }
            _ => None,
        };
        let command = signed_command_to_send(context, common, body, signature).await?;
        if let Some((receiver, amount)) = receiver {
            warn_if_below_account_creation_fee(context, receiver, amount).await;
        }
        inject_tx(command, context).await
    }

    /// Send a delegation transaction
    ///
    /// # Arguments
    /// - `input`: Delegation details including delegator, delegate, fee, and memo
    /// - `signature`: Transaction signature
    ///
    /// # Returns
    /// Delegation response with transaction hash and details
    async fn send_delegation(
        input: user_command::InputGraphQLDelegation,
        signature: Option<user_command::UserCommandSignature>,
        context: &Context,
    ) -> juniper::FieldResult<user_command::GraphQLSendDelegationResponse> {
        context.ensure_tx_submission()?;
        let signature = signature.ok_or_else(|| field_error(NO_UNLOCKED_KEY))?;
        let (common, body) = input.parse().map_err(field_error)?;
        let command = signed_command_to_send(context, common, body, signature).await?;
        inject_tx(command, context).await
    }
}

/// OCaml's answer to an unsigned send for a key it doesn't hold; this node
/// holds no keys.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L500-L505>
const NO_UNLOCKED_KEY: &str = "Couldn't find an unlocked key for specified `sender`. Did you \
     unlock the account you're making a transaction from?";

fn field_error(message: impl AsRef<str>) -> FieldError {
    FieldError::new(message.as_ref(), graphql_value!(null))
}

/// Builds the signed command `sendPayment` or `sendDelegation` submits, with
/// OCaml's checks in OCaml's order: the node is not bootstrapping, the nonce
/// is inferred or in range, the signature is valid for this network.
///
/// OCaml: `Mina_commands.setup_and_submit_user_command` and
/// `User_command_input.to_user_command`,
/// <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/user_command_input/user_command_input.ml#L200-L232>
async fn signed_command_to_send(
    context: &Context,
    common: user_command::SignedCommandInput,
    body: ledger::scan_state::transaction_logic::signed_command::Body,
    signature: user_command::UserCommandSignature,
) -> juniper::FieldResult<MinaBaseUserCommandStableV2> {
    use ledger::scan_state::currency::Nonce;

    let signature: mina_signer::Signature = signature.try_into()?;
    let from = user_command::compressed(&common.from).map_err(field_error)?;
    let account_id = ledger::AccountId::new(from, ledger::TokenId::default());

    let nonces: mina_node::rpc::RpcTransactionNonceGetResponse = context
        .rpc_sender
        .oneshot_request(RpcRequest::TransactionNonceGet(account_id))
        .await
        .ok_or(Error::StateMachineEmptyResponse)?;
    if nonces.bootstrapping {
        return Err(field_error(
            mina_node::rpc::TRANSACTION_INJECT_BOOTSTRAPPING,
        ));
    }

    let accounts: Vec<Account> = context
        .rpc_sender
        .oneshot_request(RpcRequest::LedgerAccountsGet(
            AccountQuery::PubKeyWithTokenId(common.from.clone(), TokenIdKeyHash::default()),
        ))
        .await
        .ok_or(Error::StateMachineEmptyResponse)?;
    let ledger_nonce = accounts.first().map(|account| account.nonce);

    // OCaml's `get_current_nonce`: the pool's, else the ledger's; the minimum
    // is the ledger's.
    let inferred = nonces
        .pool_nonce
        .map(Nonce::from_u32)
        .or(ledger_nonce)
        .ok_or_else(|| {
            field_error(user_command::creation_error(
                user_command::CANNOT_INFER_NONCE,
            ))
        })?;
    let min = ledger_nonce.unwrap_or(inferred);
    let nonce = user_command::choose_nonce(common.nonce, min, inferred)
        .map_err(|e| field_error(user_command::creation_error(&e)))?;

    let command = common
        .command(nonce, body, signature)
        .map_err(field_error)?;
    user_command::check_signature(&command)
        .map_err(|e| field_error(user_command::creation_error(&e)))?;

    Ok(MinaBaseUserCommandStableV2::SignedCommand(
        (&command).into(),
    ))
}

/// OCaml logs a warning when a payment to an account that doesn't exist yet
/// can't pay the account creation fee: it will be included, as failed.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/user_command_input/user_command_input.ml#L171-L198>
async fn warn_if_below_account_creation_fee(
    context: &Context,
    receiver: mina_signer::CompressedPubKey,
    amount: ledger::scan_state::currency::Amount,
) {
    let creation_fee = mina_core::constants::constraint_constants().account_creation_fee;
    if amount.as_u64() >= creation_fee {
        return;
    }
    let receiver_key = AccountPublicKey::from(receiver);
    let accounts: Option<Vec<Account>> = context
        .rpc_sender
        .oneshot_request(RpcRequest::LedgerAccountsGet(
            AccountQuery::PubKeyWithTokenId(receiver_key.clone(), TokenIdKeyHash::default()),
        ))
        .await;
    if accounts.is_some_and(|accounts| accounts.is_empty()) {
        tracing::warn!(
            "A transaction was submitted that is likely to fail because the receiver \
             account doesn't appear to have been created already and the transaction \
             amount of {} is smaller than the account creation fee of {}.",
            user_command::to_mina_string(amount.as_u64()),
            user_command::to_mina_string(creation_fee)
        );
    }
}

/// Helper function used by [`Query::pooled_user_commands`] and
/// [`Query::pooled_zkapp_commands`] to parse public key, transaction hashes and
/// command ids
/// A `publicKey` argument, refused as OCaml's `PublicKey` scalar refuses
/// one that does not decode.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2479-L2494>
fn public_key_argument(
    public_key: &PublicKeyScalar,
    field_name: &str,
) -> Result<AccountPublicKey, FieldError> {
    bounded(&public_key.0, limits::PUBLIC_KEY)
        .ok()
        .and_then(|key| AccountPublicKey::from_str(key).ok())
        // An x outside the field decodes but is no key; the ledger would
        // panic on it.
        .filter(|key| CompressedPubKey::try_from(key.clone()).is_ok())
        .ok_or_else(|| {
            ocaml_errors::argument_error("publicKey", "PublicKey", field_name, &public_key.0)
        })
}

/// A `TokenId` argument, refused as OCaml's `TokenId` scalar refuses one
/// that does not decode.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2505-L2520>
fn token_id_argument(
    token: &TokenIdScalar,
    arg_name: &str,
    field_name: &str,
) -> Result<TokenIdKeyHash, FieldError> {
    bounded(&token.0, limits::TOKEN_ID)
        .ok()
        .and_then(|token| TokenIdKeyHash::from_str(token).ok())
        // A value outside the field decodes but is no token id; the ledger
        // would panic on it.
        .filter(|token| ledger::TokenId::try_from(token).is_ok())
        .ok_or_else(|| ocaml_errors::argument_error(arg_name, "TokenId", field_name, &token.0))
}

fn parse_pooled_commands_query<CommandId, F>(
    public_key: Option<AccountPublicKey>,
    hashes: Option<Vec<String>>,
    ids: Option<Vec<ID>>,
    id_max_len: usize,
    id_map_fn: F,
) -> Result<PooledCommandsQuery<CommandId>, ConversionError>
where
    F: Fn(&str) -> Result<CommandId, conv::Error>,
{
    let hashes = match hashes {
        Some(hashes) => Some(
            hashes
                .into_iter()
                .map(|tx| -> Result<_, ConversionError> {
                    Ok(TransactionHash::from_str(bounded(
                        &tx,
                        limits::TRANSACTION_HASH,
                    )?)?)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };

    let ids = match ids {
        Some(ids) => Some(
            ids.into_iter()
                .map(|id| -> Result<_, ConversionError> {
                    Ok(id_map_fn(bounded(&id, id_max_len)?)?)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };

    Ok(PooledCommandsQuery {
        public_key,
        hashes,
        ids,
    })
}

#[cfg(test)]
mod input_tests;
