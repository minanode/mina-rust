use console::style;
use std::{fmt::Display, process};

pub fn exit_with_error<E: Display>(error: E) -> ! {
    exit_with_error_code(error, 1)
}

pub fn exit_with_error_code<E: Display>(error: E, code: i32) -> ! {
    eprintln!("{} {:#}", style("[ERROR]").red().bold(), error,);
    process::exit(code)
}
