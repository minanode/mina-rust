use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::{fs, io::Write, path::Path};

use ledger::{
    proofs::snark_worker::{
        check_work, is_zkapp_command, prove_work, spec_jobs, WorkProveError, WorkProvers,
    },
    scan_state::scan_state::transaction_snark::SokMessage,
};
use mina_node::{
    core::{channels::mpsc, snark::Snark},
    event_source::ExternalSnarkWorkerEvent,
    external_snark_worker::{
        ExternalSnarkWorkerError, ExternalSnarkWorkerWorkError, SnarkWorkSpec, SnarkWorkSpecError,
    },
    snark::{TransactionVerifier, VerifierSRS},
};
#[cfg(not(target_family = "wasm"))]
use mina_p2p_messages::binprot::BinProtWrite;
use mina_p2p_messages::v2;
use mina_signer::CompressedPubKey;

use crate::{
    block_producer::isolated::{work_kinds, BridgeInput, IsolatedBlockProver, WorkChecker},
    NodeService,
};

use super::EventSender;

pub struct SnarkWorker {
    backend: Backend,
}

enum Backend {
    /// A proving thread in this process's memory: the native node.
    Thread(mpsc::UnboundedSender<Cmd>),
    /// The web node's isolated prover (P5.3), shared with its block
    /// producer: the node's memory never holds the transaction provers.
    Isolated(IsolatedBlockProver),
}

enum Cmd {
    Submit(Box<SnarkWorkSpec>),
    Cancel,
    Kill,
}

impl SnarkWorker {
    fn send(&self, cmd: Cmd) -> Result<(), ExternalSnarkWorkerError> {
        let sent = match &self.backend {
            Backend::Thread(sender) => sender.send(cmd).is_ok(),
            Backend::Isolated(prover) => prover.worker_input(match cmd {
                Cmd::Submit(spec) => BridgeInput::SubmitWork(spec),
                Cmd::Cancel => BridgeInput::CancelWork,
                Cmd::Kill => BridgeInput::KillWorker,
            }),
        };
        sent.then_some(())
            .ok_or(ExternalSnarkWorkerError::NotRunning)
    }
}

impl mina_node::service::ExternalSnarkWorkerService for NodeService {
    fn start(
        &mut self,
        pub_key: v2::NonZeroCurvePoint,
        fee: v2::CurrencyFeeStableV1,
        work_verifier: TransactionVerifier,
        verifier_srs: Arc<VerifierSRS>,
    ) -> Result<(), ExternalSnarkWorkerError> {
        if self.replayer.is_some() {
            return Ok(());
        }
        if let Some(prover) = &self.isolated_prover {
            // The bridge answers `Started`, or `Error` if it cannot prove.
            let checker = WorkChecker {
                fee,
                prover: pub_key,
                verifier: work_verifier,
                srs: verifier_srs,
            };
            if !prover.worker_input(BridgeInput::StartWorker(Box::new(checker))) {
                return Err(ExternalSnarkWorkerError::Error(
                    "the isolated prover's bridge thread is gone".to_owned(),
                ));
            }
            self.snark_worker = Some(SnarkWorker {
                backend: Backend::Isolated(prover.clone()),
            });
            return Ok(());
        }
        let (cmd_sender, cmd_receiver) = mpsc::unbounded_channel();
        let prover = CompressedPubKey::from_address(&pub_key.to_string())
            .map_err(|err| ExternalSnarkWorkerError::Error(format!("snarker key: {err}")))?;
        let sok_message = SokMessage::create((&fee).into(), prover);
        self.snark_worker = Some(SnarkWorker {
            backend: Backend::Thread(cmd_sender),
        });
        let event_sender = self.event_sender().clone();
        let worker = Worker {
            sok_message,
            snarker: pub_key,
            fee,
            work_verifier,
            verifier_srs,
        };

        mina_node::core::thread::Builder::new()
            .name("snark_worker".to_owned())
            .spawn(move || worker_thread(cmd_receiver, event_sender, worker))
            .map(|_| ())
            .map_err(|err| ExternalSnarkWorkerError::Error(err.to_string()))
    }

    fn kill(&mut self) -> Result<(), ExternalSnarkWorkerError> {
        if self.replayer.is_some() {
            return Ok(());
        }

        self.snark_worker
            .as_ref()
            .ok_or(ExternalSnarkWorkerError::NotRunning)?
            .send(Cmd::Kill)
    }

    fn submit(&mut self, spec: SnarkWorkSpec) -> Result<(), ExternalSnarkWorkerError> {
        if self.replayer.is_some() {
            return Ok(());
        }

        self.snark_worker
            .as_ref()
            .ok_or(ExternalSnarkWorkerError::NotRunning)?
            .send(Cmd::Submit(spec.into()))
    }

    fn cancel(&mut self) -> Result<(), ExternalSnarkWorkerError> {
        if self.replayer.is_some() {
            return Ok(());
        }

        // Neither backend can stop a proof it has started: the thread
        // answers once it is done, the isolated prover drops only a job it
        // has not sent yet.
        self.snark_worker
            .as_ref()
            .ok_or(ExternalSnarkWorkerError::NotRunning)?
            .send(Cmd::Cancel)
    }

    fn save_setting(&mut self, setting: Option<&mina_node::SnarkerSetting>) {
        if self.replayer.is_some() {
            return;
        }
        let Some(files) = &self.snark_worker_files else {
            return;
        };
        if let Err(error) = files.save_setting(setting) {
            mina_core::error!(
                mina_core::log::system_time();
                kind = "SnarkWorkerSettingNotSaved",
                summary = "the worker setting was not saved: a restart won't restore it",
                path = files.setting_path().display().to_string(),
                error = error.to_string(),
            );
        }
    }

    fn save_included_work(&mut self, log: &mina_node::external_snark_worker::IncludedWorkLog) {
        if self.replayer.is_some() {
            return;
        }
        let Some(files) = &mut self.snark_worker_files else {
            return;
        };
        if let Err(error) = files.save_included_work(log) {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "SnarkWorkerIncludedWorkNotSaved",
                summary = "the included-work log was not saved",
                path = files.included_work_path().display().to_string(),
                error = error.to_string(),
            );
        }
    }
}

/// What the worker thread proves for, and checks its proofs with.
struct Worker {
    sok_message: SokMessage,
    snarker: v2::NonZeroCurvePoint,
    fee: v2::CurrencyFeeStableV1,
    work_verifier: TransactionVerifier,
    verifier_srs: Arc<VerifierSRS>,
}

fn worker_thread(
    mut cmd_receiver: mpsc::UnboundedReceiver<Cmd>,
    event_sender: EventSender,
    worker: Worker,
) {
    let _ = event_sender.send(ExternalSnarkWorkerEvent::Started.into());
    let rss_before = rss_kib();
    let started = redux::Instant::now();
    // A circuit-load error stops the worker and remains visible in its
    // dashboard status, instead of leaving a dead thread behind.
    let provers = match WorkProvers::make(Some(worker.work_verifier.clone())) {
        Ok(provers) => provers,
        Err(cause) => {
            stop_for_prover_error(
                &event_sender,
                format!("could not make the transaction provers: {cause:#}"),
            );
            return;
        }
    };
    mina_core::info!(
        mina_core::log::system_time();
        kind = "SnarkWorkerProversLoaded",
        summary = format!("transaction provers made in {} ms", started.elapsed().as_millis()),
        load_ms = started.elapsed().as_millis() as u64,
        rss_before_kib = rss_before.unwrap_or_default(),
        rss_after_kib = rss_kib().unwrap_or_default(),
    );
    while let Some(cmd) = cmd_receiver.blocking_recv() {
        match cmd {
            Cmd::Kill => {
                let _ = event_sender.send(ExternalSnarkWorkerEvent::Killed.into());
                return;
            }
            Cmd::Cancel => {
                // can't cancel as it's a blocking thread. Once this
                // is moved to another process, kill it. The proof of the
                // job, if it was submitted, has been sent before this, and
                // the node offers a proof whenever it arrives.
                let _ = event_sender.send(ExternalSnarkWorkerEvent::WorkCancelled.into());
            }
            Cmd::Submit(spec) => {
                if !provers.zkapp_loaded() && spec_jobs(&spec).into_iter().any(is_zkapp_command) {
                    if let Err(error) = load_zkapp_provers(&provers) {
                        stop_for_prover_error(&event_sender, error);
                        return;
                    }
                }
                let outcome = contain_panic(|| prove_and_check(&provers, &spec, &worker));
                let event = match outcome {
                    Err(err) => {
                        #[cfg(not(target_family = "wasm"))]
                        capture_failed_work(&spec, &worker.sok_message, &err);
                        mina_core::warn!(
                            mina_core::log::system_time();
                            kind = "SnarkWorkerWorkFailed",
                            summary = format!("no proof of {}; job dropped", work_kinds(&spec)),
                            jobs = work_kinds(&spec),
                            error = err.to_string(),
                        );
                        ExternalSnarkWorkerEvent::WorkError(err)
                    }
                    Ok(res) => {
                        #[cfg(not(target_family = "wasm"))]
                        capture_proved_zkapp_work(&spec, &worker.sok_message);
                        ExternalSnarkWorkerEvent::WorkResult(res)
                    }
                };

                let _ = event_sender.send(event.into());
            }
        }
    }
}

/// Saves the complete wire spec (including the statement, witness, ledger and
/// zkApp data) and SOK message only when the operator opts in. Files are
/// private because a work spec can contain account data.
#[cfg(not(target_family = "wasm"))]
fn capture_failed_work(
    spec: &SnarkWorkSpec,
    message: &SokMessage,
    error: &ExternalSnarkWorkerWorkError,
) {
    let Some(dir) = std::env::var_os("MINA_SNARK_WORKER_CAPTURE_DIR") else {
        return;
    };
    if let Err(cause) = write_work(Path::new(&dir), spec, message, Some(error)) {
        mina_core::warn!(
            mina_core::log::system_time();
            kind = "SnarkWorkerCaptureFailed",
            error = cause.to_string(),
        );
    }
}

/// With `MINA_SNARK_WORKER_CAPTURE_ZKAPP=1` as well, saves every zkApp
/// command job that proved too, as `<id>.proved.bin`, to audit which zkApps
/// the worker proves (ZK1).
#[cfg(not(target_family = "wasm"))]
fn capture_proved_zkapp_work(spec: &SnarkWorkSpec, message: &SokMessage) {
    if !capture_zkapp_enabled(std::env::var_os("MINA_SNARK_WORKER_CAPTURE_ZKAPP").as_deref())
        || !spec_jobs(spec).into_iter().any(is_zkapp_command)
    {
        return;
    }
    let Some(dir) = std::env::var_os("MINA_SNARK_WORKER_CAPTURE_DIR") else {
        return;
    };
    if let Err(cause) = write_work(Path::new(&dir), spec, message, None) {
        mina_core::warn!(
            mina_core::log::system_time();
            kind = "SnarkWorkerCaptureFailed",
            error = cause.to_string(),
        );
    }
}

/// Only `1` turns the capture of proved zkApp jobs on: `0`, any other value
/// or none leaves it off.
#[cfg(not(target_family = "wasm"))]
fn capture_zkapp_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

/// Creates the capture directory, and any parent it needs, readable by its
/// owner only. An existing directory keeps its mode.
#[cfg(not(target_family = "wasm"))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Creates a new capture file readable by its owner only: a work spec holds
/// the ledger accounts its transactions touch, and an error can quote them.
#[cfg(not(target_family = "wasm"))]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes `<id>.bin` and `<id>.error.txt` for a failed job, `<id>.proved.bin`
/// for one that proved.
#[cfg(not(target_family = "wasm"))]
fn write_work(
    dir: &Path,
    spec: &SnarkWorkSpec,
    message: &SokMessage,
    error: Option<&ExternalSnarkWorkerWorkError>,
) -> std::io::Result<()> {
    create_private_dir(dir)?;
    let id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos(),
        std::process::id()
    );
    let path = dir.join(match error {
        Some(_) => format!("{id}.bin"),
        None => format!("{id}.proved.bin"),
    });
    let mut file = create_private_file(&path)?;
    spec.binprot_write(&mut file)?;
    let sok: v2::MinaBaseSokMessageStableV1 = message.into();
    sok.binprot_write(&mut file)?;
    file.flush()?;
    match error {
        Some(error) => {
            let mut text = create_private_file(&dir.join(format!("{id}.error.txt")))?;
            text.write_all(error.to_string().as_bytes())?;
            mina_core::info!(
                mina_core::log::system_time();
                kind = "SnarkWorkerFailureCaptured",
                path = path.display().to_string(),
            );
        }
        None => mina_core::debug!(
            mina_core::log::system_time();
            kind = "SnarkWorkerZkappCaptured",
            path = path.display().to_string(),
        ),
    }
    Ok(())
}

fn stop_for_prover_error(event_sender: &EventSender, error: String) {
    mina_core::error!(
        mina_core::log::system_time();
        kind = "SnarkWorkerProversFailed",
        summary = "the SNARK worker stopped: its provers could not be made",
        error = error.clone(),
    );
    let _ = event_sender
        .send(ExternalSnarkWorkerEvent::Error(ExternalSnarkWorkerError::Error(error)).into());
}

/// Runs one job's proving; a panic in a prover (P5.1: a failed zkApp command
/// whose proved update has no verification key) becomes this job's error, so
/// the job is dropped and the worker and node keep running.
fn contain_panic(
    prove: impl FnOnce() -> Result<Snark, ExternalSnarkWorkerWorkError>,
) -> Result<Snark, ExternalSnarkWorkerWorkError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(prove)).unwrap_or_else(|payload| {
        Err(ExternalSnarkWorkerWorkError::Error(format!(
            "prover panicked: {}",
            panic_message(&*payload)
        )))
    })
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// Makes the zkApp step provers, on the first zkApp command job, and logs
/// what that cost.
fn load_zkapp_provers(provers: &WorkProvers) -> Result<(), String> {
    let rss_before = rss_kib();
    let started = redux::Instant::now();
    provers
        .zkapp()
        .map_err(|err| format!("could not make zkApp provers: {err:#}"))?;
    let load_ms = started.elapsed().as_millis() as u64;
    mina_core::info!(
        mina_core::log::system_time();
        kind = "SnarkWorkerZkappProversLoaded",
        summary = format!("zkApp provers made in {load_ms} ms"),
        load_ms = load_ms,
        rss_before_kib = rss_before.unwrap_or_default(),
        rss_after_kib = rss_kib().unwrap_or_default(),
    );
    Ok(())
}

/// This process's resident memory, where Linux reports it.
fn rss_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Proves `spec`, then checks the proofs as an OCaml snark pool would check
/// our diff ([`check_proved_work`]).
fn prove_and_check(
    provers: &WorkProvers,
    spec: &SnarkWorkSpec,
    worker: &Worker,
) -> Result<Snark, ExternalSnarkWorkerWorkError> {
    let kinds = work_kinds(spec);
    let started = redux::Instant::now();
    let proved = prove_work(spec, &worker.sok_message, provers).map_err(|err| match err {
        WorkProveError::InvalidStatement => {
            ExternalSnarkWorkerWorkError::WorkSpecError(SnarkWorkSpecError::InvalidBigInt)
        }
        err => ExternalSnarkWorkerWorkError::Error(err.to_string()),
    })?;
    let times = WorkTimes {
        kinds,
        prove_ms: started.elapsed().as_millis() as u64,
        per_job_ms: proved
            .prove_times
            .iter()
            .map(|time| time.as_millis() as u64)
            .collect(),
        queued_ms: 0,
    };
    let work = Snark {
        snarker: worker.snarker.clone(),
        fee: worker.fee.clone(),
        proofs: proved.proofs.into(),
    };
    check_proved_work(work, times, &worker.work_verifier, &worker.verifier_srs)
}

/// How long a bundle took, for the log.
pub struct WorkTimes {
    /// `base`, `merge+merge`, ...
    pub kinds: String,
    /// The whole bundle: in the isolated prover, from dispatch to response.
    pub prove_ms: u64,
    /// Each job in the prover (witness, step and wrap).
    pub per_job_ms: Vec<u64>,
    /// How long the bundle waited for the isolated prover; 0 in-process.
    pub queued_ms: u64,
}

/// Checks our own work as an OCaml snark pool would check our diff: every
/// proof verifies with this network's transaction key, for the sok digest of
/// our fee and key. Work that fails is never offered: OCaml peers record
/// `Sent_invalid_proof` against whoever sends an invalid proof. Both the
/// in-process worker and the isolated prover's bridge (P5.3) use this. The
/// work is returned whole: the node offers it only for the key it was made
/// for, at the fee its sok digest commits to (P5.4).
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L319-L476>
pub fn check_proved_work(
    work: Snark,
    times: WorkTimes,
    verifier: &TransactionVerifier,
    srs: &VerifierSRS,
) -> Result<Snark, ExternalSnarkWorkerWorkError> {
    let WorkTimes {
        kinds,
        prove_ms,
        per_job_ms,
        queued_ms,
    } = times;
    let checked = redux::Instant::now();
    let check = check_work(&work, verifier, srs);
    let check_ms = checked.elapsed().as_millis() as u64;
    let per_job_ms = per_job_ms
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    match check {
        Ok(()) => {
            mina_core::debug!(
                mina_core::log::system_time();
                kind = "SnarkWorkerWorkProved",
                summary = format!("proved and checked {kinds} in {prove_ms} + {check_ms} ms"),
                job_id = display(work.job_id()),
                jobs = kinds,
                prove_ms = prove_ms,
                per_job_ms = per_job_ms,
                check_ms = check_ms,
                queued_ms = queued_ms,
            );
            Ok(work)
        }
        Err(err) => {
            mina_core::error!(
                mina_core::log::system_time();
                kind = "SnarkWorkerSelfCheckFailed",
                summary = format!("our proof of {kinds} failed its own check; not offered"),
                job_id = display(work.job_id()),
                error = err.to_string(),
            );
            Err(ExternalSnarkWorkerWorkError::Error(format!(
                "own proof failed verification: {err}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prover_load_failure_stops_worker_with_visible_reason() {
        use mina_node::event_source::Event;

        let (sender, mut receiver) = mpsc::unbounded_channel();
        let reason = "could not make the transaction provers: cannot load circuit file 4.0.2devnet/step_transaction_gates.json: test circuit blob missing";
        stop_for_prover_error(&sender, reason.to_owned());
        match receiver.try_recv().unwrap() {
            Event::ExternalSnarkWorker(ExternalSnarkWorkerEvent::Error(
                ExternalSnarkWorkerError::Error(error),
            )) => assert_eq!(error, reason),
            event => panic!("expected worker startup error, got {event:?}"),
        }
    }

    #[test]
    fn a_prover_panic_is_the_jobs_error() {
        let outcome = contain_panic(|| {
            let missing: Option<()> = None;
            missing.expect("verification key");
            unreachable!()
        });
        assert_eq!(
            outcome.unwrap_err().to_string(),
            ExternalSnarkWorkerWorkError::Error("prover panicked: verification key".to_owned())
                .to_string()
        );
        // The thread that caught it goes on: the next job's outcome is its own.
        assert!(matches!(
            contain_panic(|| Err(ExternalSnarkWorkerWorkError::Cancelled)),
            Err(ExternalSnarkWorkerWorkError::Cancelled)
        ));
    }

    #[test]
    fn only_1_captures_proved_zkapp_jobs() {
        use std::ffi::OsStr;

        assert!(capture_zkapp_enabled(Some(OsStr::new("1"))));
        for off in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
            assert!(!capture_zkapp_enabled(off.map(OsStr::new)), "{off:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn captures_are_readable_by_their_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("captures").join("zk1");
        create_private_dir(&dir).unwrap();
        for path in [root.path().join("captures"), dir.clone()] {
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{path:?}");
        }
        for name in ["job.bin", "job.error.txt"] {
            let mut file = create_private_file(&dir.join(name)).unwrap();
            file.write_all(b"capture").unwrap();
            let mode = fs::metadata(dir.join(name)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
        // A capture never replaces an earlier one.
        assert!(create_private_file(&dir.join("job.bin")).is_err());
    }
}
