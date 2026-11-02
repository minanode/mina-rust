//! `mina internal verify-uptime-work` rejects bad input with exit 1, never a
//! panic, and leaves no diagnostics directory behind.
use std::{path::Path, process::Command};

use base64::{engine::general_purpose::STANDARD, Engine};

/// An OCaml 4.0.3 `Proof_data`, captured from a devnet uptime report.
fn ocaml_proof_data() -> Vec<u8> {
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/uptime-fixtures/full-proof/devnet-581352-signed/uptime-1.json"
    ))
    .unwrap();
    STANDARD
        .decode(report["data"]["snark_work"].as_str().unwrap())
        .unwrap()
}

fn verify(network: &str, input: &Path) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_mina"))
        .env_clear()
        .args(["--network", network, "internal", "verify-uptime-work"])
        .arg(input)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_rejected(network: &str, input: &Path, hint: &str) {
    let (code, stdout, stderr) = verify(network, input);
    assert_eq!(code, Some(1), "{}: {stderr}", input.display());
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains(hint), "{}: {stderr}", input.display());
    assert!(stderr.contains(&input.display().to_string()), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// Only the input files remain in `dir`: no diagnostics directory leaks.
fn assert_only(dir: &Path, names: &[&str]) {
    let mut found = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    found.sort();
    let mut names = names.to_vec();
    names.sort();
    assert_eq!(found, names);
}

#[test]
fn bad_inputs_exit_one_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let original = ocaml_proof_data();
    let mut trailing = original.clone();
    trailing.push(0);
    let mut oversized = original.clone();
    oversized.resize(4 * 1024 * 1024 + 1, 0);
    let files: [(&str, &[u8], &str); 5] = [
        ("empty.bin", &[], "invalid uptime work Bin_prot"),
        (
            "truncated.bin",
            &original[..original.len() / 2],
            "invalid uptime work Bin_prot",
        ),
        ("garbage.bin", &[0xfc; 64], "invalid uptime work Bin_prot"),
        (
            "trailing.bin",
            &trailing,
            "trailing bytes after uptime work",
        ),
        ("oversized.bin", &oversized, "is larger than"),
    ];
    for (name, bytes, _) in &files {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    for (name, _, hint) in &files {
        assert_rejected("devnet", &dir.path().join(name), hint);
    }
    // The filesystem root has no parent; a directory and a device are not
    // files. Each was rejected before any diagnostics directory is made.
    assert_rejected("devnet", Path::new("/"), "is not a file");
    assert_rejected("devnet", dir.path(), "is not a file");
    assert_rejected("devnet", Path::new("/dev/null"), "is not a file");
    assert_rejected(
        "devnet",
        &dir.path().join("missing.bin"),
        "cannot open uptime work file",
    );
    let mut names = files.iter().map(|(name, ..)| *name).collect::<Vec<_>>();
    assert_only(dir.path(), &names);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let unreadable = dir.path().join("unreadable.bin");
        std::fs::write(&unreadable, &original).unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o200)).unwrap();
        // Root reads any file regardless of its mode.
        if std::fs::read(&unreadable).is_err() {
            assert_rejected("devnet", &unreadable, "cannot read uptime work file");
        }
        names.push("unreadable.bin");
        assert_only(dir.path(), &names);
    }
}

#[test]
fn unknown_network_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("work.bin");
    std::fs::write(&input, ocaml_proof_data()).unwrap();
    let (code, _, stderr) = verify("nonet", &input);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("invalid value 'nonet'"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// The captured devnet work verifies on devnet and fails under mainnet's
/// keys, each removing its diagnostics directory.
#[test]
fn ocaml_work_verifies_only_on_its_network() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("work.bin");
    std::fs::write(&input, ocaml_proof_data()).unwrap();
    let (code, stdout, stderr) = verify("devnet", &input);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "uptime work proof verified\n");
    assert_rejected("mainnet", &input, "uptime work proof failed verification");
    assert_only(dir.path(), &["work.bin"]);
}

/// A complete `Proof_data` whose wrap challenge polynomial commitment is
/// (0, 0), the point at infinity: bytes 1256 to 1320 of the captured work.
/// A malformed proof fails verification with exit 1.
#[test]
fn a_commitment_at_infinity_fails_verification() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("zero-commitment.bin");
    let mut bytes = ocaml_proof_data();
    assert!(bytes[1256..1320].iter().any(|&b| b != 0));
    bytes[1256..1320].fill(0);
    std::fs::write(&input, &bytes).unwrap();
    assert_rejected("devnet", &input, "uptime work proof failed verification");
    assert_only(dir.path(), &["zero-commitment.bin"]);
}
