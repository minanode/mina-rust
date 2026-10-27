pub mod commands;
#[cfg(not(target_family = "wasm"))]
pub mod panic_hook;

mod exit_with_error;
pub use exit_with_error::{exit_with_error, exit_with_error_code};
