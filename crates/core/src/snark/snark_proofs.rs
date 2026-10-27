//! PC1: the proofs of a SNARK work, decoded or cached.
//!
//! A work's proofs arrive decoded. The snark pool keeps the works it stores
//! as OCaml does (`Ledger_proof.Cached`): each proof's statement decoded, for
//! job ids, matching and ordering, and the proof itself as binprot bytes in
//! the [`proof_cache`](super::proof_cache). [`SnarkProofs::get`] decodes it
//! again, for block production, serving and gossip. Binprot, serde and
//! equality see the same value either way.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/ledger_proof/ledger_proof.ml#L35-L60>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_work/transaction_snark_work.ml#L106-L136>

use std::sync::Arc;

use malloc_size_of::MallocSizeOf;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2::{
        LedgerProofProdStableV2, MinaStateSnarkedLedgerStateWithSokStableV2,
        TransactionSnarkProofStableV2, TransactionSnarkStableV2,
        TransactionSnarkWorkTStableV2Proofs,
    },
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    proof_cache::{CachedBlob, ProofStore},
    SnarkJobId,
};

#[derive(Clone)]
pub struct SnarkProofs(Repr);

#[derive(Clone)]
enum Repr {
    Decoded(Arc<TransactionSnarkWorkTStableV2Proofs>),
    Cached(Arc<Cached>),
}

/// One or two statements, each with its proof's bytes.
struct Cached {
    first: (MinaStateSnarkedLedgerStateWithSokStableV2, CachedBlob),
    second: Option<(MinaStateSnarkedLedgerStateWithSokStableV2, CachedBlob)>,
}

/// A cached proof that could not be read back or decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cached proof unavailable: {0}")]
pub struct ProofUnavailable(pub String);

impl SnarkProofs {
    pub fn new(proofs: TransactionSnarkWorkTStableV2Proofs) -> Self {
        Self(Repr::Decoded(Arc::new(proofs)))
    }

    /// The proofs, decoded: shared if they are, else read from the cache.
    pub fn get(&self) -> Result<Arc<TransactionSnarkWorkTStableV2Proofs>, ProofUnavailable> {
        match &self.0 {
            Repr::Decoded(proofs) => {
                // PC2 handles can have been cached through a shared block
                // or scan state even when this outer work is still decoded.
                for p in ledger_proofs(proofs) {
                    if let Some(blob) = p.0.proof.cached_blob() {
                        blob.bytes().map_err(|e| ProofUnavailable(e.to_string()))?;
                    }
                }
                Ok(proofs.clone())
            }
            Repr::Cached(cached) => {
                let read = |(statement, blob): &(_, CachedBlob)| {
                    let bytes = blob.bytes().map_err(|e| ProofUnavailable(e.to_string()))?;
                    let proof = TransactionSnarkProofStableV2::binprot_read(&mut &bytes[..])
                        .map_err(|e| ProofUnavailable(e.to_string()))?;
                    Ok::<_, ProofUnavailable>(LedgerProofProdStableV2(TransactionSnarkStableV2 {
                        statement: MinaStateSnarkedLedgerStateWithSokStableV2::clone(statement),
                        proof: proof.into(),
                    }))
                };
                let first = read(&cached.first)?;
                Ok(Arc::new(match &cached.second {
                    None => TransactionSnarkWorkTStableV2Proofs::One(first),
                    Some(second) => {
                        TransactionSnarkWorkTStableV2Proofs::Two((first, read(second)?))
                    }
                }))
            }
        }
    }

    /// These proofs with their bytes in `store`, and only their statements
    /// decoded. Proofs already cached stay as they are.
    pub fn cache_in(&self, store: &ProofStore) -> Self {
        let Repr::Decoded(proofs) = &self.0 else {
            return self.clone();
        };
        let put = |proof: &LedgerProofProdStableV2| {
            let mut bytes = Vec::with_capacity(12 * 1024);
            // A nested PC2 handle can fail to read even though Vec writes
            // themselves cannot fail.
            proof.0.proof.binprot_write(&mut bytes)?;
            Ok::<_, std::io::Error>((proof.0.statement.clone(), store.put(bytes)))
        };
        let result = (|| {
            Ok::<_, std::io::Error>(match &**proofs {
                TransactionSnarkWorkTStableV2Proofs::One(p) => (put(p)?, None),
                TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => (put(p)?, Some(put(q)?)),
            })
        })();
        let (first, second) = match result {
            Ok(proofs) => proofs,
            Err(error) => {
                crate::log::inner::warn!("proof cache: {error}");
                return self.clone();
            }
        };
        Self(Repr::Cached(Arc::new(Cached { first, second })))
    }

    /// As [`Self::cache_in`], in the node's store.
    pub fn cache(&self) -> Self {
        self.cache_in(super::proof_cache::global())
    }

    pub fn is_cached(&self) -> bool {
        matches!(self.0, Repr::Cached(_))
    }

    /// Whether a read of a cached proof failed (missing file, other length or
    /// other bytes than were written): such a work is not to be used again.
    pub fn is_unreadable(&self) -> bool {
        match &self.0 {
            Repr::Decoded(proofs) => ledger_proofs(proofs).any(|p| p.0.proof.is_unreadable()),
            Repr::Cached(_) => self.cached_blobs().any(CachedBlob::is_unreadable),
        }
    }

    /// The statements, one or two, without reading a cached proof.
    pub fn statements(
        &self,
    ) -> (
        &MinaStateSnarkedLedgerStateWithSokStableV2,
        Option<&MinaStateSnarkedLedgerStateWithSokStableV2>,
    ) {
        match &self.0 {
            Repr::Decoded(proofs) => match &**proofs {
                TransactionSnarkWorkTStableV2Proofs::One(p) => (&p.0.statement, None),
                TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => {
                    (&p.0.statement, Some(&q.0.statement))
                }
            },
            Repr::Cached(cached) => (&cached.first.0, cached.second.as_ref().map(|s| &s.0)),
        }
    }

    pub fn job_id(&self) -> SnarkJobId {
        let (first, second) = self.statements();
        (&first.source, &second.unwrap_or(first).target).into()
    }

    /// The cached proofs' blobs, if cached: for memory accounting.
    pub fn cached_blobs(&self) -> impl Iterator<Item = &CachedBlob> {
        let cached = match &self.0 {
            Repr::Decoded(_) => None,
            Repr::Cached(cached) => Some(cached),
        };
        cached
            .into_iter()
            .flat_map(|c| std::iter::once(&c.first.1).chain(c.second.as_ref().map(|s| &s.1)))
    }

    /// The decoded proofs, if they are decoded.
    pub fn decoded(&self) -> Option<&TransactionSnarkWorkTStableV2Proofs> {
        match &self.0 {
            Repr::Decoded(proofs) => Some(proofs),
            Repr::Cached(_) => None,
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Repr::Decoded(a), Repr::Decoded(b)) => Arc::ptr_eq(a, b),
            (Repr::Cached(a), Repr::Cached(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The address of the shared value, to count it once.
    pub fn as_ptr(&self) -> *const () {
        match &self.0 {
            Repr::Decoded(proofs) => Arc::as_ptr(proofs) as *const (),
            Repr::Cached(cached) => Arc::as_ptr(cached) as *const (),
        }
    }

    /// The bytes of each proof, cached or encoded now.
    fn proof_bytes(&self) -> Result<Vec<Vec<u8>>, ProofUnavailable> {
        match &self.0 {
            Repr::Decoded(proofs) => Ok(match &**proofs {
                TransactionSnarkWorkTStableV2Proofs::One(p) => vec![try_encode(&p.0.proof)?],
                TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => {
                    vec![try_encode(&p.0.proof)?, try_encode(&q.0.proof)?]
                }
            }),
            Repr::Cached(_) => self
                .cached_blobs()
                .map(|blob| {
                    blob.bytes()
                        .map(|b| b.into_owned())
                        .map_err(|e| ProofUnavailable(e.to_string()))
                })
                .collect(),
        }
    }
}

fn ledger_proofs(
    proofs: &TransactionSnarkWorkTStableV2Proofs,
) -> impl Iterator<Item = &LedgerProofProdStableV2> {
    let (first, second) = match proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => (p, None),
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => (p, Some(q)),
    };
    std::iter::once(first).chain(second)
}

fn try_encode(value: &impl BinProtWrite) -> Result<Vec<u8>, ProofUnavailable> {
    let mut bytes = Vec::new();
    value
        .binprot_write(&mut bytes)
        .map_err(|e| ProofUnavailable(e.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = value.binprot_write(&mut bytes);
    bytes
}

impl From<TransactionSnarkWorkTStableV2Proofs> for SnarkProofs {
    fn from(proofs: TransactionSnarkWorkTStableV2Proofs) -> Self {
        Self::new(proofs)
    }
}

impl From<Arc<TransactionSnarkWorkTStableV2Proofs>> for SnarkProofs {
    fn from(proofs: Arc<TransactionSnarkWorkTStableV2Proofs>) -> Self {
        Self(Repr::Decoded(proofs))
    }
}

/// Equal when the statements and the proofs' bytes are: what equality of the
/// decoded value means. A cached proof that cannot be read equals nothing
/// but itself.
impl PartialEq for SnarkProofs {
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        if let (Some(a), Some(b)) = (self.decoded(), other.decoded()) {
            return a == b;
        }
        self.statements() == other.statements()
            && match (self.proof_bytes(), other.proof_bytes()) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            }
    }
}

impl std::fmt::Debug for SnarkProofs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Repr::Decoded(proofs) => proofs.fmt(f),
            Repr::Cached(cached) => f
                .debug_struct("CachedProofs")
                .field("statements", &self.statements())
                .field("first", &cached.first.1)
                .field("second", &cached.second.as_ref().map(|s| &s.1))
                .finish(),
        }
    }
}

impl BinProtWrite for SnarkProofs {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.get()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .binprot_write(w)
    }
}

impl BinProtRead for SnarkProofs {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        TransactionSnarkWorkTStableV2Proofs::binprot_read(r).map(Self::new)
    }
}

impl Serialize for SnarkProofs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.get()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SnarkProofs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        TransactionSnarkWorkTStableV2Proofs::deserialize(deserializer).map(Self::new)
    }
}

/// Heap bytes: the decoded proofs, or the statements and in-memory blobs.
impl MallocSizeOf for SnarkProofs {
    fn size_of(&self, ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        if ops.have_seen_ptr(self.as_ptr()) {
            return 0;
        }
        match &self.0 {
            Repr::Decoded(proofs) => {
                size_of::<TransactionSnarkWorkTStableV2Proofs>() + (**proofs).size_of(ops)
            }
            Repr::Cached(cached) => {
                let (first, second) = self.statements();
                size_of::<Cached>()
                    + first.size_of(ops)
                    + second.map_or(0, |s| s.size_of(ops))
                    + std::iter::once(&cached.first.1)
                        .chain(cached.second.as_ref().map(|s| &s.1))
                        .map(CachedBlob::heap_bytes)
                        .sum::<usize>()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mina_p2p_messages::{
        rpc::GetCompletedSnarksV1,
        rpc_kernel::{ResponsePayload, RpcMethod},
        v2::TransactionSnarkWorkTStableV2,
    };

    use super::*;
    use crate::snark::Snark;

    /// The 32 two-proof Mesa mainnet bundles an OCaml 4.0.x seed served.
    fn works() -> Vec<TransactionSnarkWorkTStableV2> {
        let bytes =
            include_bytes!("../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin");
        type Response = <GetCompletedSnarksV1 as RpcMethod>::Response;
        let payload = ResponsePayload::<Response>::binprot_read(&mut &bytes[..]).unwrap();
        payload.0.unwrap().0.unwrap().0.into_iter().collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mina-snark-proofs-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn files(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |d| d.count())
    }

    #[test]
    fn a_cached_work_is_the_same_work() {
        let dir = scratch("same");
        let stores = [ProofStore::memory(), ProofStore::dir(&dir).unwrap()];
        for store in &stores {
            for work in works() {
                let decoded = Snark::from(work.clone());
                let cached = Snark {
                    proofs: decoded.proofs.cache_in(store),
                    ..decoded.clone()
                };
                assert!(cached.proofs.is_cached());
                assert_eq!(cached.proofs.cached_blobs().count(), 2);
                assert_eq!(cached.job_id(), decoded.job_id());
                assert_eq!(cached.statement(), decoded.statement());
                assert_eq!(cached.to_wire().unwrap(), work);
                assert_eq!(cached.proofs, decoded.proofs);
                assert_eq!(decoded.proofs, cached.proofs);
                // Binprot and serde give the decoded work's bytes.
                let (mut a, mut b) = (Vec::new(), Vec::new());
                cached.binprot_write(&mut a).unwrap();
                decoded.binprot_write(&mut b).unwrap();
                assert_eq!(a, b);
                assert_eq!(
                    serde_json::to_string(&cached).unwrap(),
                    serde_json::to_string(&decoded).unwrap()
                );
                // Caching twice keeps the same blobs.
                assert!(cached.proofs.cache_in(store).ptr_eq(&cached.proofs));
            }
        }
        // The files went with the works.
        assert_eq!(files(&dir), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cached_proofs_take_files_not_heap_until_dropped() {
        let dir = scratch("files");
        let store = ProofStore::dir(&dir).unwrap();
        let all = works();
        let decoded: Vec<Snark> = all.iter().cloned().map(Snark::from).collect();
        let cached: Vec<SnarkProofs> = decoded.iter().map(|s| s.proofs.cache_in(&store)).collect();
        assert_eq!(files(&dir), 2 * all.len());
        let stats = store.stats();
        assert_eq!(stats.files, 2 * all.len() as u64);
        assert!(stats.file_bytes > 10_000 * stats.files);
        for (c, d) in cached.iter().zip(&decoded) {
            let heap = |p: &SnarkProofs| {
                let mut ops = malloc_size_of::MallocSizeOfOps::new(None, Some(Box::new(|_| false)));
                p.size_of(&mut ops)
            };
            // Statements stay; the two ~11 KB proofs go.
            assert!(
                heap(c) * 5 < heap(&d.proofs),
                "{} {}",
                heap(c),
                heap(&d.proofs)
            );
        }
        let clones = cached.clone();
        drop(cached);
        assert_eq!(files(&dir), 2 * all.len());
        drop(clones);
        assert_eq!(files(&dir), 0);
        assert_eq!(store.stats().files, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lost_cached_proof_is_an_error_and_equals_only_itself() {
        let dir = scratch("lost");
        let store = ProofStore::dir(&dir).unwrap();
        let decoded = Snark::from(works().remove(0));
        let cached = decoded.proofs.cache_in(&store);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(cached.get().is_err());
        assert!(cached.binprot_write(&mut Vec::new()).is_err());
        assert!(serde_json::to_string(&cached).is_err());
        assert_ne!(cached, decoded.proofs);
        assert_eq!(cached, cached.clone());
        // Statements need no read.
        assert_eq!(cached.job_id(), decoded.job_id());
    }

    /// The bytes of the two cached proofs' files, oldest first.
    fn proof_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort_by_key(|p| {
            p.file_stem()
                .and_then(|s| s.to_str()?.parse::<u64>().ok())
                .unwrap()
        });
        files
    }

    /// Another proof, valid binprot of the same length: the second proof's
    /// `ft_eval1` set to its `z_1`.
    fn same_length_other_proof(bytes: &[u8]) -> Vec<u8> {
        let mut proof = TransactionSnarkProofStableV2::binprot_read(&mut &bytes[..]).unwrap();
        proof.0.proof.ft_eval1 = proof.0.proof.bulletproof.z_1.clone();
        let other = encode(&proof);
        assert_eq!(other.len(), bytes.len());
        assert_ne!(other, bytes);
        other
    }

    #[test]
    fn pc2_nested_cached_proofs_fail_closed_in_a_decoded_work() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-data/proof-cache/nested-pool-{}",
            std::process::id()
        ));
        let store = ProofStore::dir(&dir).unwrap();
        let work = works().remove(0);
        let proofs = SnarkProofs::new(work.proofs.clone());
        let independent = SnarkProofs::new(works().remove(0).proofs);
        ledger_proofs(&work.proofs)
            .next()
            .unwrap()
            .0
            .proof
            .cache_in(&store)
            .unwrap();
        assert!(!proofs.is_cached());
        assert!(proofs.get().is_ok());
        let file = proof_files(&dir).remove(0);
        let bytes = std::fs::read(&file).unwrap();
        std::fs::write(file, same_length_other_proof(&bytes)).unwrap();
        assert_not_used(&proofs, &independent);
        let failed_cache = proofs.cache_in(&store);
        assert_not_used(&failed_cache, &independent);
        assert_eq!(proofs.job_id(), independent.job_id());
        drop((failed_cache, proofs, work));
        assert_eq!(store.stats().files, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn assert_not_used(cached: &SnarkProofs, decoded: &SnarkProofs) {
        assert!(cached.get().is_err());
        assert!(cached.is_unreadable());
        assert!(cached.binprot_write(&mut Vec::new()).is_err());
        assert!(serde_json::to_string(cached).is_err());
        assert_ne!(cached, decoded);
        // Statements need no read: the work is still identifiable.
        assert_eq!(cached.job_id(), decoded.job_id());
    }

    /// Review fix: a cached file swapped for another proof that decodes, of
    /// the same length, is caught by the digest and never used.
    #[test]
    fn a_swapped_cached_proof_that_still_decodes_is_not_used() {
        let dir = scratch("swapped");
        let store = ProofStore::dir(&dir).unwrap();
        for (i, work) in works().into_iter().take(4).enumerate() {
            let decoded = Snark::from(work).proofs;
            let cached = decoded.cache_in(&store);
            let files = proof_files(&dir);
            let file = &files[files.len() - 1 - i % 2];
            let swapped = same_length_other_proof(&std::fs::read(file).unwrap());
            // It decodes: only the digest tells it from the proof verified.
            assert!(TransactionSnarkProofStableV2::binprot_read(&mut &swapped[..]).is_ok());
            std::fs::write(file, &swapped).unwrap();
            assert_not_used(&cached, &decoded);
            assert!(cached.cache_in(&store).is_unreadable());
        }
        assert_eq!(store.stats().unreadable, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Review fix: a corrupted cached file whose bytes do not decode is not
    /// used either.
    #[test]
    fn a_corrupted_cached_proof_is_not_used() {
        let dir = scratch("corrupted");
        let store = ProofStore::dir(&dir).unwrap();
        let decoded = Snark::from(works().remove(0)).proofs;
        let cached = decoded.cache_in(&store);
        let file = &proof_files(&dir)[0];
        let len = std::fs::metadata(file).unwrap().len() as usize;
        let corrupted = vec![0xff; len];
        assert!(TransactionSnarkProofStableV2::binprot_read(&mut &corrupted[..]).is_err());
        std::fs::write(file, &corrupted).unwrap();
        assert_not_used(&cached, &decoded);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_different_proof_is_not_equal() {
        let store = ProofStore::memory();
        let mut work = works().remove(0);
        let original = Snark::from(work.clone()).proofs.cache_in(&store);
        if let TransactionSnarkWorkTStableV2Proofs::Two((_, q)) = &mut work.proofs {
            let mut proof = (*q.0.proof.get().unwrap()).clone();
            proof.0.proof.ft_eval1 = proof.0.proof.bulletproof.z_1.clone();
            q.0.proof = proof.into();
        }
        let changed = Snark::from(work).proofs;
        assert_eq!(original.job_id(), changed.job_id());
        assert_ne!(original, changed);
        assert_ne!(original, changed.cache_in(&store));
    }
}
