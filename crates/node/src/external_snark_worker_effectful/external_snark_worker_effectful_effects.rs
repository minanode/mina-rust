use super::ExternalSnarkWorkerEffectfulAction;
use crate::{external_snark_worker::ExternalSnarkWorkerError, ExternalSnarkWorkerAction};
use redux::ActionWithMeta;

pub fn external_snark_worker_effectful_effects<S: crate::Service>(
    store: &mut crate::Store<S>,
    action: ActionWithMeta<ExternalSnarkWorkerEffectfulAction>,
) {
    let (action, _) = action.split();
    match action {
        ExternalSnarkWorkerEffectfulAction::Start { public_key, fee } => {
            let work_verify = &store.state().snark.work_verify;
            let work_verifier = work_verify.verifier_index.clone();
            let verifier_srs = work_verify.verifier_srs.clone();
            if let Err(err) = store
                .service
                .start(public_key, fee, work_verifier, verifier_srs)
            {
                store.dispatch(ExternalSnarkWorkerAction::Error {
                    error: err,
                    permanent: true,
                });
            }
        }
        ExternalSnarkWorkerEffectfulAction::Kill => match store.service().kill() {
            Ok(()) => {}
            // Nothing to stop: stopped (P5.4). An error would kill it again,
            // and again.
            Err(ExternalSnarkWorkerError::NotRunning) => {
                store.dispatch(ExternalSnarkWorkerAction::Killed);
            }
            Err(err) => {
                store.dispatch(ExternalSnarkWorkerAction::Error {
                    error: err,
                    permanent: true,
                });
            }
        },
        ExternalSnarkWorkerEffectfulAction::SubmitWork { spec } => {
            if let Err(err) = store.service().submit(*spec) {
                store.dispatch(ExternalSnarkWorkerAction::WorkError { error: err.into() });
            }
        }
        ExternalSnarkWorkerEffectfulAction::SaveSetting { setting } => {
            store.service().save_setting(setting.as_ref());
        }
        ExternalSnarkWorkerEffectfulAction::CancelWork => {
            if let Err(error) = store.service().cancel() {
                store.dispatch(ExternalSnarkWorkerAction::Error {
                    error,
                    permanent: true,
                });
            }
        }
    }
}
