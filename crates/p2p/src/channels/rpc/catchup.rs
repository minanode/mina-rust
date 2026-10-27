//! Inbound OCaml 4.0.2 bootstrap/catch-up RPCs. The network dispatcher gates
//! these on `P2pCatchupRpcConfig` before they enter the RPC channel; the
//! default serves every peer so missing gossip ancestors remain fetchable.

use binprot_derive::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{common::StateHashV1, list::List, rpc, rpc_kernel::RpcMethod};
use serde::{Deserialize, Serialize};

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/transition_frontier.ml#L34>
pub const MAX_CATCHUP_CHAIN_LENGTH: usize = 20;

/// OCaml sync ledger uses `query_peer'` with up to 100 parallel queries.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/mina_networking.ml#L441-L453>
pub const MAX_CATCHUP_RPC_REQUESTS: usize = 100;

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum CatchupRequest {
    Ancestry(Box<rpc::GetAncestryV2Query>),
    TransitionChain(List<StateHashV1>),
    TransitionChainProof(StateHashV1),
    TransitionKnowledge,
}

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub enum CatchupResponse {
    Ancestry(rpc::GetAncestryV2Response),
    TransitionChain(<rpc::GetTransitionChainV2 as RpcMethod>::Response),
    TransitionChainProof(rpc::GetTransitionChainProofV1ForV2Response),
    TransitionKnowledge(<rpc::GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response),
}

impl CatchupResponse {
    /// The enum is internal only: send the method's payload without its enum
    /// discriminant, inside Async RPC's result and length wrappers.
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Ancestry(r) => response_bytes(r),
            Self::TransitionChain(r) => response_bytes(r),
            Self::TransitionChainProof(r) => response_bytes(r),
            Self::TransitionKnowledge(r) => response_bytes(r),
        }
    }
}

pub fn response_bytes<T: binprot::BinProtWrite>(response: T) -> Vec<u8> {
    use binprot::BinProtWrite;
    use mina_p2p_messages::rpc_kernel::{NeedsLength, ResponsePayload, RpcResult};
    let payload: ResponsePayload<T> = RpcResult(Ok(NeedsLength(response)));
    let mut bytes = Vec::new();
    if let Err(error) = payload.binprot_write(&mut bytes) {
        mina_core::log::inner::warn!("RPC response unavailable: {error}");
        // Discard partial bytes and send a valid Async RPC error. This
        // constructor has no Sexp argument in OCaml either.
        bytes.clear();
        let unavailable: ResponsePayload<()> =
            RpcResult(Err(mina_p2p_messages::rpc_kernel::Error::Connection_closed));
        let _ = unavailable.binprot_write(&mut bytes);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use binprot::BinProtRead;
    use mina_p2p_messages::rpc_kernel::ResponsePayload;

    #[test]
    fn unavailable_and_empty_responses_have_ocaml_option_and_list_tags() {
        // Async RPC Ok = 0, payload length = 1, option None/list [] = 0.
        assert_eq!(CatchupResponse::Ancestry(None).into_bytes(), [0, 1, 0]);
        assert_eq!(
            CatchupResponse::TransitionChain(None).into_bytes(),
            [0, 1, 0]
        );
        assert_eq!(
            CatchupResponse::TransitionChainProof(None).into_bytes(),
            [0, 1, 0]
        );
        assert_eq!(
            CatchupResponse::TransitionKnowledge(List::new()).into_bytes(),
            [0, 1, 0]
        );
        // Some [] has a distinct encoding: Some = 1 followed by [] = 0.
        assert_eq!(
            CatchupResponse::TransitionChain(Some(List::new())).into_bytes(),
            [0, 2, 1, 0]
        );
    }

    #[test]
    fn chain_and_ancestry_encoding_matches_captured_ocaml_bytes() {
        let bytes = include_bytes!(
            "../../../../p2p-messages/tests/files/mesa/get_transition_chain_v2_563900_zkapp.bin"
        );
        let chain =
            ResponsePayload::<<rpc::GetTransitionChainV2 as RpcMethod>::Response>::binprot_read(
                &mut &bytes[..],
            )
            .unwrap()
            .0
            .unwrap()
            .0;
        assert_eq!(CatchupResponse::TransitionChain(chain).into_bytes(), bytes);
        let bytes = include_bytes!("../../../../p2p-messages/tests/files/mesa/get_ancestry_v3.bin");
        let ancestry = ResponsePayload::<rpc::GetAncestryV2Response>::binprot_read(&mut &bytes[..])
            .unwrap()
            .0
            .unwrap()
            .0;
        assert_eq!(CatchupResponse::Ancestry(ancestry).into_bytes(), bytes);
    }
    #[test]
    fn proof_and_knowledge_encoding_matches_captured_ocaml_bytes() {
        let bytes = include_bytes!(
            "../../../../p2p-messages/tests/files/mesa/get_transition_chain_proof_v1.bin"
        );
        let response =
            ResponsePayload::<rpc::GetTransitionChainProofV1ForV2Response>::binprot_read(
                &mut &bytes[..],
            )
            .unwrap()
            .0
            .unwrap()
            .0;
        assert_eq!(
            CatchupResponse::TransitionChainProof(response).into_bytes(),
            bytes
        );
        let bytes = include_bytes!(
            "../../../../p2p-messages/tests/files/mesa/get_transition_knowledge_v1.bin"
        );
        let response = ResponsePayload::<<rpc::GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response>::binprot_read(&mut &bytes[..]).unwrap().0.unwrap().0;
        assert_eq!(
            CatchupResponse::TransitionKnowledge(response).into_bytes(),
            bytes
        );
    }
}
