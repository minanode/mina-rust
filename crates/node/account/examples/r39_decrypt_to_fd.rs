//! Decrypt R39's throwaway key into a private pipe for the browser harness.
//!
//! The key is never written to stdout, stderr, the environment, or a file.

use std::{env, fs, io::Write, os::fd::FromRawFd};

use mina_node_account::AccountSecretKey;

const SENDER: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("expected encrypted key and password file paths".into());
    }
    let password = fs::read_to_string(&args[2])?;
    let key =
        AccountSecretKey::from_encrypted_file(&args[1], password.trim_end_matches(['\r', '\n']))?;
    if key.public_key().to_string() != SENDER {
        return Err("decrypted key is not the R39 throwaway account".into());
    }

    // SAFETY: the harness starts this process with a pipe at descriptor 3.
    let mut pipe = unsafe { fs::File::from_raw_fd(3) };
    pipe.write_all(key.to_string().as_bytes())?;
    pipe.flush()?;
    Ok(())
}
