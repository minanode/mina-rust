use backtrace::Backtrace;
use std::panic::PanicHookInfo;

use crate::commands::report::crash;

/// Mimic default hook:
/// <https://github.com/rust-lang/rust/blob/5986ff05d8480da038dd161b3a6aa79ff364a851/library/std/src/panicking.rs#L246>
///
/// Unlike the default hook, this one allocates.
/// We store (+ display) panics in non-main threads, and display them all when the main thread panics.
/// Once the node has started, each panic is also written to a crash file in
/// its work directory, for `mina report`.
pub fn new_hook(info: &PanicHookInfo<'_>) {
    use std::{any::Any, io::Write};

    fn payload_as_str(payload: &dyn Any) -> &str {
        if let Some(&s) = payload.downcast_ref::<&'static str>() {
            s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Box<dyn Any>"
        }
    }

    static PREVIOUS_PANICS: std::sync::Mutex<Vec<Vec<u8>>> =
        const { std::sync::Mutex::new(Vec::new()) };

    let mut s: Vec<u8> = Vec::with_capacity(64 * 1024);
    let backtrace = Backtrace::new();

    let current = std::thread::current();
    let name = current.name().unwrap_or("<unnamed>");
    let location = info.location().unwrap();
    let msg = payload_as_str(info.payload());

    let _ = writeln!(&mut s, "\nthread '{name}' panicked at {location}:\n{msg}");
    let _ = writeln!(&mut s, "{:#?}", &backtrace);

    eprintln!("{}", String::from_utf8_lossy(&s));

    if let Some(path) = crash::record_panic(name, msg, Some(location), &backtrace) {
        eprintln!(
            "Crash file written to {}: run `mina report` to file a bug report.",
            path.display()
        );
    }

    if name != "main" {
        let Ok(mut previous) = PREVIOUS_PANICS.lock() else {
            return;
        };
        // Make sure we don't store too many panics
        if previous.len() < 256 {
            previous.push(s);
            eprintln!("Saved panic from thread '{name}'");
        } else {
            eprintln!("Panic from thread '{name}' not saved !");
        }
    } else {
        let Ok(previous) = PREVIOUS_PANICS.lock() else {
            return;
        };
        eprintln!("\nNumber of panics from others threads: {}", previous.len());
        for panic in previous.iter() {
            eprintln!("{}", String::from_utf8_lossy(panic));
        }
    }
}
