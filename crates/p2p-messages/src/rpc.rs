//! Mina RPC methods
use std::collections::BTreeMap;

use binprot::{BinProtRead, BinProtWrite};
use binprot_derive::{BinProtRead, BinProtWrite};
use serde::{Deserialize, Serialize};

use crate::{
    common::*,
    core,
    core::InetAddrV1Versioned,
    list::List,
    rpc_kernel::*,
    string::CharString,
    v2,
    v2::MinaBaseSparseLedgerBaseStableV2,
    versioned::{Ver, Versioned},
};

macro_rules! mina_rpc {
    ($name:ident, $tag:literal, $version:literal, $query:ty, $response:ty $(,)?) => {
        #[derive(Debug)]
        pub struct $name;
        impl crate::rpc_kernel::RpcMethod for $name {
            const NAME: crate::rpc_kernel::RpcTag = $tag.as_bytes();
            const NAME_STR: &'static str = $tag;
            const VERSION: crate::versioned::Ver = $version;
            type Query = $query;
            type Response = $response;
        }
    };
}

mina_rpc!(
    SendArchiveDiffUnversioned,
    "Send_archive_diff",
    0,
    v2::ArchiveRpc,
    ()
);

mina_rpc!(
    VersionedRpcMenuV1,
    "__Versioned_rpc.Menu",
    1,
    (),
    List<(CharString, Ver)>
);

/// The menu an OCaml 4.0.2 node answers `__Versioned_rpc.Menu` with: every
/// RPC it implements and its version, in the order it registers them.
///
/// OCaml asks for the menu on every RPC stream before its query, and chooses a
/// version from it. We answer with the same menu, so that OCaml sends its
/// queries as it would to another OCaml node. RPCs we don't serve yet are
/// answered `Unimplemented_rpc`, as async_rpc_kernel does for a tag it doesn't
/// know when it keeps the connection.
///
/// The menu lists the implementations given to `Versioned_rpc.Menu.add`, in
/// order and without the menu itself, which async_rpc_kernel v0.14 appends
/// after them. Mina 4.0.2 builds that list from `all_rpcs`, one version each:
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L1203-L1214>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/gossip_net/libp2p.ml#L352-L373>
/// OCaml: <https://github.com/janestreet/async_rpc_kernel/blob/v0.14.0/src/versioned_rpc.ml#L601-L619>
pub const VERSIONED_RPC_MENU: [(&str, Ver); 10] = [
    (
        GetSomeInitialPeersV1ForV2::NAME_STR,
        GetSomeInitialPeersV1ForV2::VERSION,
    ),
    (
        GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME_STR,
        GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
    ),
    (
        AnswerSyncLedgerQueryV2::NAME_STR,
        AnswerSyncLedgerQueryV2::VERSION,
    ),
    (GetBestTipV2::NAME_STR, GetBestTipV2::VERSION),
    (GetAncestryV2::NAME_STR, GetAncestryV2::VERSION),
    (
        GetTransitionKnowledgeV1ForV2::NAME_STR,
        GetTransitionKnowledgeV1ForV2::VERSION,
    ),
    (
        GetTransitionChainV2::NAME_STR,
        GetTransitionChainV2::VERSION,
    ),
    (
        GetTransitionChainProofV1ForV2::NAME_STR,
        GetTransitionChainProofV1ForV2::VERSION,
    ),
    (BanNotifyV1::NAME_STR, BanNotifyV1::VERSION),
    (
        GetCompletedSnarksV1::NAME_STR,
        GetCompletedSnarksV1::VERSION,
    ),
];

impl VersionedRpcMenuV1 {
    /// The answer to a menu query: [`VERSIONED_RPC_MENU`].
    pub fn menu() -> <Self as RpcMethod>::Response {
        VERSIONED_RPC_MENU
            .iter()
            .map(|(tag, version)| (CharString::from(*tag), *version))
            .collect()
    }
}

mina_rpc!(
    GetSomeInitialPeersV1ForV2,
    "get_some_initial_peers",
    1,
    (),
    List<v2::NetworkPeerPeerStableV1>
);

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L748-L838>
pub const MAX_COMPLETED_SNARKS: usize = 32;
/// Local transport bound, including the RPC envelope (32 pairs of proofs).
pub const MAX_COMPLETED_SNARKS_BYTES: usize = 8 * 1024 * 1024;

/// Same encoding as an OCaml list, rejecting its length before reading proofs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, BinProtWrite)]
pub struct CompletedSnarks(pub List<v2::TransactionSnarkWorkTStableV2>);

impl BinProtRead for CompletedSnarks {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error> {
        let binprot::Nat0(len) = binprot::Nat0::binprot_read(r)?;
        if len > MAX_COMPLETED_SNARKS as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "get_completed_snarks response exceeds OCaml's 32-work limit",
            )
            .into());
        }
        let mut works = List::new();
        for _ in 0..len {
            works.push_back(v2::TransactionSnarkWorkTStableV2::binprot_read(r)?);
        }
        Ok(Self(works))
    }
}

mina_rpc!(
    GetCompletedSnarksV1,
    "get_completed_snarks",
    1,
    (),
    Option<CompletedSnarks>
);

pub type GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response = Option<(
    v2::TransactionSnarkScanStateStableV2,
    LedgerHashV1,
    v2::MinaBasePendingCoinbaseStableV2,
    List<v2::MinaStateProtocolStateValueStableV2>,
)>;

// Mesa nodes serve only version 3, whose scan state keeps each transaction with
// its status instead of the applied transaction, and plain ledger proofs in
// merge jobs.
//
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L253-L289>
mina_rpc!(
    GetStagedLedgerAuxAndPendingCoinbasesAtHashV2,
    "get_staged_ledger_aux_and_pending_coinbases_at_hash",
    3,
    StateHashV1,
    GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response,
);

// Mesa nodes serve only version 5, where child-hash queries carry a subtree
// depth and answers list the subtree's hashes.
//
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L13-L74>
mina_rpc!(
    AnswerSyncLedgerQueryV2,
    "answer_sync_ledger_query",
    5,
    (LedgerHashV1, v2::MinaLedgerSyncLedgerQueryStableV1),
    RpcResult<v2::MinaLedgerSyncLedgerAnswerStableV2, core::Error>
);

mina_rpc!(
    GetTransitionChainV2,
    "get_transition_chain",
    2,
    List<StateHashV1>,
    Option<List<v2::MinaBlockBlockStableV2>>
);

pub type GetTransitionChainProofV1ForV2Response = Option<(StateHashV1, List<StateBodyHashV1>)>;
mina_rpc!(
    GetTransitionChainProofV1ForV2,
    "get_transition_chain_proof",
    1,
    StateHashV1,
    GetTransitionChainProofV1ForV2Response,
);

mina_rpc!(
    GetTransitionKnowledgeV1,
    "Get_transition_knowledge",
    1,
    (),
    List<StateHashV1Versioned>
);

mina_rpc!(
    GetTransitionKnowledgeV1ForV2,
    "Get_transition_knowledge",
    1,
    (),
    List<StateHashV1>
);

// pub struct ConsensusDataConsensusStateValue;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct WithHashV1<A, H> {
    pub data: A,
    pub hash: H,
}
pub type WithHashV1Versioned<A, H> = Versioned<WithHashV1<A, H>, 1>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct ProofCarryingDataWithHashV1<A, B> {
    pub data: A,
    pub proof: B,
}
pub type ProofCarryingDataWithHashV1Versioned<A, B> =
    Versioned<ProofCarryingDataWithHashV1<A, B>, 1>;

pub type GetAncestryV2Query =
    WithHashV1<v2::ConsensusProofOfStakeDataConsensusStateValueStableV2, StateHashV1>;
pub type GetAncestryV2Response = Option<
    ProofCarryingDataWithHashV1<
        v2::MinaBlockBlockStableV2,
        (List<StateBodyHashV1>, v2::MinaBlockBlockStableV2),
    >,
>;
// Mesa nodes serve only version 3. Its wire format is unchanged for blocks
// without zkApp commands.
//
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L885-L918>
mina_rpc!(
    GetAncestryV2,
    "get_ancestry",
    3,
    GetAncestryV2Query,
    GetAncestryV2Response,
);

mina_rpc!(BanNotifyV1, "ban_notify", 1, core::Time, ());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct ProofCarryingDataStableV1<A, B> {
    pub data: A,
    pub proof: B,
}
pub type ProofCarryingDataStableV1Versioned<A, B> = Versioned<ProofCarryingDataStableV1<A, B>, 1>;
pub type GetBestTipV2Response = Option<
    ProofCarryingDataStableV1<
        v2::MinaBlockBlockStableV2,
        (
            List<v2::MinaBaseStateBodyHashStableV1>,
            v2::MinaBlockBlockStableV2,
        ),
    >,
>;
// Mesa nodes serve only version 3. Its wire format is unchanged for blocks
// without zkApp commands.
//
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L1096-L1125>
mina_rpc!(GetBestTipV2, "get_best_tip", 3, (), GetBestTipV2Response);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct NodeStatusV2 {
    node_ip_addr: InetAddrV1Versioned,
    node_peer_id: v2::NetworkPeerPeerIdStableV1,
    sync_status: v2::SyncStatusTStableV1,
    peers: List<v2::NetworkPeerPeerIdStableV1>,
    block_producers: List<v2::NonZeroCurvePoint>,
    protocol_state_hash: v2::StateHash,
    ban_statuses: List<(
        v2::NetworkPeerPeerIdStableV1,
        v2::TrustSystemPeerStatusStableV1,
    )>,
    k_block_hashes_and_timestamps: List<(v2::StateHash, CharString)>,
    git_commit: CharString,
    uptime_minutes: i32,
    block_height_opt: Option<i32>,
}
mina_rpc!(GetNodeStatusV2, "get_node_status", 2, (), RpcResult<NodeStatusV2, core::Error>);

mina_rpc!(GetEpochLedgerV2, "get_epoch_ledger", 2, LedgerHashV1, RpcResult<MinaBaseSparseLedgerBaseStableV2, CharString>);

/// Registry for uniformly JSONifying RPC payload data.
///
/// ```
/// let r = mina_p2p_messages::JSONifyPayloadRegistry::new();
/// let mut d = &b"\x01\x00"[..];
/// let jsonifier = r.get(b"get_some_initial_peers", 1).unwrap();
/// let json = jsonifier.read_query(&mut d).unwrap();
/// ```
pub struct JSONifyPayloadRegistry {
    table: BTreeMap<(&'static [u8], Ver), Box<dyn JSONinifyPayloadReader>>,
}

impl Default for JSONifyPayloadRegistry {
    fn default() -> Self {
        Self::v2()
    }
}

impl JSONifyPayloadRegistry {
    #[deprecated = "Use `[v2]` method instead."]
    pub fn new() -> Self {
        Self::v2()
    }

    /// Creates registry with Mina V2 specific RPC methods.
    pub fn v2() -> Self {
        let mut this = Self {
            table: BTreeMap::new(),
        };
        this.insert(VersionedRpcMenuV1);
        this.insert(GetSomeInitialPeersV1ForV2);
        this.insert(GetStagedLedgerAuxAndPendingCoinbasesAtHashV2);
        this.insert(AnswerSyncLedgerQueryV2);
        this.insert(GetTransitionChainV2);
        this.insert(GetTransitionChainProofV1ForV2);
        this.insert(GetTransitionKnowledgeV1ForV2);
        this.insert(BanNotifyV1);
        this.insert(GetAncestryV2);
        this.insert(GetBestTipV2);
        this.insert(GetNodeStatusV2);
        this.insert(GetEpochLedgerV2);
        this
    }

    pub fn get<'a, 'b: 'a>(
        &'a self,
        name: &'b [u8],
        version: Ver,
    ) -> Option<&'a dyn JSONinifyPayloadReader> {
        self.table.get(&(name, version)).map(Box::as_ref)
    }

    fn insert<T>(&mut self, t: T)
    where
        T: RpcMethod + 'static,
        T::Query: Serialize,
        T::Response: Serialize,
    {
        self.table.insert((T::NAME, T::VERSION), Box::new(t));
    }
}

#[cfg(test)]
mod tests {
    use crate::JSONifyPayloadRegistry;

    /// The menu is OCaml 4.0.2's, entry for entry, in its order.
    #[test]
    fn versioned_rpc_menu_is_ocaml_4_0_2s() {
        assert_eq!(
            super::VERSIONED_RPC_MENU,
            [
                ("get_some_initial_peers", 1),
                ("get_staged_ledger_aux_and_pending_coinbases_at_hash", 3),
                ("answer_sync_ledger_query", 5),
                ("get_best_tip", 3),
                ("get_ancestry", 3),
                ("Get_transition_knowledge", 1),
                ("get_transition_chain", 2),
                ("get_transition_chain_proof", 1),
                ("ban_notify", 1),
                ("get_completed_snarks", 1),
            ]
        );
    }

    /// The menu answer has the wire format of the menu an older OCaml node
    /// sent (`rpc_kernel::tests::message`): `Ok`, the length of what follows,
    /// and a list of (tag, version).
    #[test]
    fn versioned_rpc_menu_response() {
        use super::{RpcMethod, VersionedRpcMenuV1};
        use crate::rpc_kernel::{Message, NeedsLength, Response, RpcResult};
        use binprot::{BinProtRead, BinProtWrite};

        let response = Message::<<VersionedRpcMenuV1 as RpcMethod>::Response>::Response(Response {
            id: 0x157ec,
            data: RpcResult(Ok(NeedsLength(VersionedRpcMenuV1::menu()))),
        });
        let mut bytes = Vec::new();
        response.binprot_write(&mut bytes).unwrap();

        // Response, id, Ok, then the payload's length in 3 bytes.
        let payload_len = bytes.len() - 10;
        assert!((0x80..0x8000).contains(&payload_len));
        let mut expected = hex::decode("02fdec57010000fe").unwrap();
        expected.extend_from_slice(&(payload_len as u16).to_le_bytes());
        expected.extend_from_slice(b"\x0a\x16get_some_initial_peers\x01");
        assert_eq!(&bytes[..expected.len()], expected.as_slice());
        assert!(bytes.ends_with(b"\x14get_completed_snarks\x01"));

        let decoded = Message::<<VersionedRpcMenuV1 as RpcMethod>::Response>::binprot_read(
            &mut bytes.as_slice(),
        )
        .unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    #[ignore = "Fix it"]
    fn jsonify_registry_content_v2() {
        let r = JSONifyPayloadRegistry::v2();
        for (name, version) in [
            ("__Versioned_rpc.Menu", 1),
            ("get_some_initial_peers", 1),
            ("get_staged_ledger_aux_and_pending_coinbases_at_hash", 2),
            ("answer_sync_ledger_query", 2),
            ("get_transition_chain", 2),
            ("get_transition_chain_proof", 1),
            ("Get_transition_knowledge", 1),
            ("get_ancestry", 2),
            ("ban_notify", 1),
            ("get_best_tip", 2),
            ("get_node_status", 2),
            ("get_epoch_ledger", 2),
        ] {
            assert!(r.get(name.as_bytes(), version).is_some());
        }
    }

    #[test]
    #[ignore = "Fix it"]
    fn jsonify_registry_query() {
        let r = JSONifyPayloadRegistry::v2();
        let payload =
            hex::decode("220101e7dd9b0d45abb2e4dec2c5d22e1f1bd8ae5133047914209a0229e90a62ecfb0e")
                .unwrap();
        let mut ptr = payload.as_slice();
        let jsonify = r.get(b"get_transition_chain", 1).unwrap();
        let json = jsonify.read_query(&mut ptr).unwrap();
        let expected = serde_json::json!([
            "0x0efbec620ae929029a201479043351aed81b1f2ed2c5c2dee4b2ab450d9bdde7"
        ]);
        assert_eq!(json, expected);
    }
}
