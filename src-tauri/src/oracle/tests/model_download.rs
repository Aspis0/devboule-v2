//! The model download state machine's idempotence: a second start must not
//! replace a download that is already in flight.

use oracle_core::CancelFlag;

use super::support::TestEnvironment;
use crate::oracle::commands::oracle_model_download_start_inner;
use crate::oracle::runtime::ORACLE_MODEL_ENV;
use crate::oracle::{OracleModelState, OracleRuntime};

#[test]
fn second_model_download_start_does_not_replace_an_in_flight_state() {
    let env = TestEnvironment::new("onnx");
    env.set(ORACLE_MODEL_ENV, "model-without-an-installer");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(temp.path().to_path_buf());
    {
        let mut state = runtime
            .model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.status.state = OracleModelState::Downloading;
        state.cancel = Some(CancelFlag::new());
        state.attempted = true;
    }

    oracle_model_download_start_inner(&runtime).expect("second start is an idempotent no-op");
    let state = runtime
        .model_download
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(state.status.state, OracleModelState::Downloading);
    assert!(state.cancel.is_some());
    assert!(state.attempted);
}
