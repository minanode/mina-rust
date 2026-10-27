use mina_core::snark::SnarkJobId;
use mina_p2p_messages::v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Instances;

pub type SnarkWorkId = SnarkJobId;

/// TODO use more slim type `OneOrTwo`...
pub type SnarkWorkSpec = SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Instances;

pub type SnarkWorkResult = mina_core::snark::SnarkProofs;
