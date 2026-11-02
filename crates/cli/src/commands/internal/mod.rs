pub mod cli_reference;
pub mod graphql;

#[derive(Debug, clap::Args)]
pub struct Internal {
    #[command(subcommand)]
    pub command: InternalCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum InternalCommand {
    /// GraphQL endpoint introspection and management.
    Graphql(graphql::Graphql),
    /// Print the command-line reference of every visible command.
    #[command(hide = true)]
    CliReference(cli_reference::CliReference),
    /// Verify an uptime Proof_data file against its original statement/digest.
    #[command(hide = true)]
    VerifyUptimeWork { proof_data: std::path::PathBuf },
}

impl Internal {
    pub fn run(self) -> anyhow::Result<()> {
        match self.command {
            InternalCommand::Graphql(v) => v.run(),
            InternalCommand::CliReference(v) => v.run(),
            InternalCommand::VerifyUptimeWork { proof_data } => rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .stack_size(64 * 1024 * 1024)
                .build()?
                .install(|| verify_uptime_work(&proof_data)),
        }
    }
}

/// Larger than any `Proof_data`: one transaction SNARK is about 11 KB.
const MAX_UPTIME_WORK_BYTES: u64 = 4 * 1024 * 1024;

fn verify_uptime_work(path: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use ledger::proofs::{verification::verify_transaction, verifiers::TransactionVerifier};
    use std::io::Read;

    // Check the input before touching the filesystem: a directory, a device
    // or the filesystem root is an error, not a panic.
    let shown = path.display();
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot open uptime work file {shown}"))?;
    anyhow::ensure!(path.is_file(), "uptime work input {shown} is not a file");
    let parent = path
        .parent()
        .with_context(|| format!("uptime work input {shown} has no parent directory"))?;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_UPTIME_WORK_BYTES + 1).read_to_end(&mut bytes))
        .with_context(|| format!("cannot read uptime work file {shown}"))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_UPTIME_WORK_BYTES,
        "uptime work file {shown} is larger than {MAX_UPTIME_WORK_BYTES} bytes"
    );
    let (statement, proof) = decode_uptime_work(&bytes)
        .with_context(|| format!("cannot decode uptime work file {shown}"))?;

    // main() has already initialized the selected network before dispatch.
    // Failed verification writes a diagnostic dump. Give it a private,
    // temporary work directory instead of requiring a running node.
    let diagnostics = tempfile::Builder::new()
        .prefix("uptime-work-verification-")
        .tempdir_in(parent)
        .with_context(|| format!("cannot create a diagnostics directory next to {shown}"))?;
    mina_core::set_work_dir(diagnostics.path().to_path_buf());
    // Verify the proof under its original digest, including OCaml's zero SOK.
    // The gate separately checks whether that digest is eligible for exception.
    anyhow::ensure!(
        verify_transaction(
            [(&statement, &*proof)],
            &TransactionVerifier::make(),
            &ledger::verifier::get_srs::<mina_signer::BaseField>(),
        ),
        "uptime work proof failed verification: {shown}"
    );
    println!("uptime work proof verified");
    Ok(())
}

/// The statement and proof of a `Proof_data` file, with no trailing bytes.
fn decode_uptime_work(
    bytes: &[u8],
) -> anyhow::Result<(
    ledger::scan_state::scan_state::transaction_snark::Statement<
        ledger::scan_state::scan_state::transaction_snark::SokDigest,
    >,
    std::sync::Arc<mina_p2p_messages::v2::TransactionSnarkProofStableV2>,
)> {
    use ledger::scan_state::scan_state::transaction_snark::{SokDigest, Statement};
    use mina_node_native::reporting::uptime_work::ProofData;
    use mina_p2p_messages::binprot::BinProtRead;

    let mut input = bytes;
    let data = ProofData::binprot_read(&mut input)
        .map_err(|e| anyhow::anyhow!("invalid uptime work Bin_prot: {e}"))?;
    anyhow::ensure!(input.is_empty(), "trailing bytes after uptime work");
    let statement = Statement::<SokDigest>::try_from(&data.proof.0.statement)
        .map_err(|_| anyhow::anyhow!("invalid uptime work statement"))?;
    let proof = data
        .proof
        .0
        .proof
        .get()
        .map_err(|e| anyhow::anyhow!("invalid uptime work proof: {e}"))?;
    Ok((statement, proof))
}

#[cfg(test)]
mod tests {
    use super::decode_uptime_work;
    use base64::{engine::general_purpose::STANDARD, Engine};

    /// An OCaml 4.0.3 `Proof_data`, captured from a devnet uptime report.
    fn ocaml_proof_data() -> Vec<u8> {
        let report: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../tools/uptime-fixtures/full-proof/devnet-581352-signed/uptime-1.json"
        ))
        .unwrap();
        STANDARD
            .decode(report["data"]["snark_work"].as_str().unwrap())
            .unwrap()
    }

    /// Truncated, overwritten or oversized-length input is an error. None of
    /// it may panic or abort on an allocation.
    #[test]
    fn malformed_proof_data_is_an_error_not_a_panic() {
        let original = ocaml_proof_data();
        decode_uptime_work(&original).unwrap();
        for end in 0..original.len() {
            assert!(
                decode_uptime_work(&original[..end]).is_err(),
                "prefix {end}"
            );
        }
        let mut trailing = original.clone();
        trailing.push(0);
        assert!(decode_uptime_work(&trailing).is_err());
        // 0xfc starts an 8-byte Bin_prot integer: a length read here asks for
        // an enormous allocation.
        let huge = [0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
        for at in 0..original.len() {
            for patch in [&[0xff][..], &huge[..]] {
                let mut bytes = original.clone();
                let end = (at + patch.len()).min(bytes.len());
                bytes[at..end].copy_from_slice(&patch[..end - at]);
                let _ = decode_uptime_work(&bytes);
            }
        }
    }
}
