pub use tracing::Level;

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::{
        fmt::Result,
        fs::{self, File, OpenOptions},
        io::{self, Write},
        path::PathBuf,
        time::{Duration, Instant},
    };
    use tracing::{field::Visit, level_filters::LevelFilter, Level};
    use tracing_appender::non_blocking::WorkerGuard;
    use tracing_subscriber::{
        field::{RecordFields, VisitOutput},
        fmt::{
            format::{PrettyVisitor, Writer},
            time::FormatTime,
            FormatFields,
        },
        layer::SubscriberExt,
        Layer,
    };

    #[allow(unused)]
    fn redux_timer(w: &mut Writer<'_>) -> Result {
        match redux::SystemTime::now().duration_since(redux::SystemTime::UNIX_EPOCH) {
            Ok(v) => {
                write!(w, "{}", v.as_nanos())
            }
            Err(_) => write!(w, "unknown-time"),
        }
    }

    #[allow(dead_code)]
    struct ReduxTimer;

    impl FormatTime for ReduxTimer {
        fn format_time(&self, w: &mut Writer<'_>) -> Result {
            redux_timer(w)
        }
    }

    struct FilterVisit<T>(T);

    impl<T> FilterVisit<T> {
        fn into_inner(self) -> T {
            self.0
        }
    }

    impl<T> Visit for FilterVisit<T>
    where
        T: Visit,
    {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if !field.name().starts_with("trace_") {
                self.0.record_debug(field, value);
            }
        }
    }

    #[derive(Default)]
    struct TracingFieldFormatter;

    impl<'writer> FormatFields<'writer> for TracingFieldFormatter {
        fn format_fields<R: RecordFields>(
            &self,
            writer: Writer<'writer>,
            fields: R,
        ) -> std::fmt::Result {
            let mut v = FilterVisit(PrettyVisitor::new(writer, true));
            fields.record(&mut v);
            v.into_inner().finish()
        }
    }

    pub fn initialize(max_log_level: Level) {
        let builder = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(max_log_level)
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
            .with_test_writer();
        //.with_timer(ReduxTimer)

        if max_log_level != Level::TRACE {
            let subscriber = builder.fmt_fields(TracingFieldFormatter).finish();
            tracing::subscriber::set_global_default(subscriber)
        } else {
            let subscriber = builder.finish();
            tracing::subscriber::set_global_default(subscriber)
        }
        .expect("global subscriber should be configurable");
    }

    /// Where the node's log file goes and how much of it is kept.
    #[derive(Debug, Clone)]
    pub struct LogFileOptions {
        pub dir: PathBuf,
        /// The current file is rotated before a write would take it past
        /// this size.
        pub max_file_bytes: u64,
        /// Files kept, the current one included: at most
        /// `max_file_bytes * max_files` bytes of logs on disk.
        pub max_files: usize,
    }

    impl LogFileOptions {
        pub const FILE_NAME: &'static str = "mina.log";
        pub const DEFAULT_MAX_FILE_MB: u64 = 20;
        pub const DEFAULT_MAX_FILES: usize = 10;
    }

    pub fn initialize_with_filesystem_output(
        max_log_level: Level,
        options: LogFileOptions,
    ) -> Option<WorkerGuard> {
        let file = match RotatingFile::open(
            options.dir,
            LogFileOptions::FILE_NAME,
            options.max_file_bytes,
            options.max_files,
        ) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("WARN file logging disabled: {error}");
                initialize(max_log_level);
                return None;
            }
        };
        let (file_writer, file_guard) = tracing_appender::non_blocking(file);
        let level_filter = LevelFilter::from_level(max_log_level);

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_writer)
            .with_ansi(false)
            .with_filter(level_filter);

        let stdout_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
            .with_filter(level_filter);

        let subscriber = tracing_subscriber::Registry::default()
            .with(file_layer)
            .with(stdout_layer);

        tracing::subscriber::set_global_default(subscriber)
            .expect("Failed to set global subscriber");

        Some(file_guard)
    }

    /// A log file rotated by size, as OCaml's `Logger.Transport.File_system.
    /// dumb_logrotate`: `NAME` is written until a write would take it past
    /// `max_bytes`; it then becomes `NAME.1`, `NAME.1` becomes `NAME.2` and so
    /// on, and the file that would be `NAME.<max_files>` is deleted. At most
    /// `max_files` files of about `max_bytes` each are kept.
    ///
    /// Each write goes to one file whole: tracing writes an event at a time,
    /// so no line is split between two files.
    pub struct RotatingFile {
        dir: PathBuf,
        name: String,
        max_bytes: u64,
        max_files: usize,
        file: Option<File>,
        size: u64,
        next_rotation_attempt: Option<Instant>,
        rotation_warning_reported: bool,
    }

    impl RotatingFile {
        pub fn open(
            dir: PathBuf,
            name: &str,
            max_bytes: u64,
            max_files: usize,
        ) -> io::Result<Self> {
            fs::create_dir_all(&dir)?;
            let mut this = Self {
                dir,
                name: name.to_owned(),
                max_bytes: max_bytes.max(1),
                max_files: max_files.max(1),
                file: None,
                size: 0,
                next_rotation_attempt: None,
                rotation_warning_reported: false,
            };
            // Files a run with a larger `max_files` left beyond the new bound.
            let mut index = this.max_files;
            while fs::remove_file(this.path(index)).is_ok() {
                index += 1;
            }
            this.open_current()?;
            Ok(this)
        }

        /// `NAME` for the current file, `NAME.<index>` for the older ones.
        pub fn path(&self, index: usize) -> PathBuf {
            match index {
                0 => self.dir.join(&self.name),
                _ => self.dir.join(format!("{}.{index}", self.name)),
            }
        }

        fn open_current(&mut self) -> io::Result<()> {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path(0))?;
            self.size = file.metadata()?.len();
            self.file = Some(file);
            Ok(())
        }

        fn rotate(&mut self) -> io::Result<()> {
            // Keep the old handle until the replacement is open. On Unix it
            // remains writable even if a rename succeeded before a later one
            // failed.
            if self.max_files == 1 {
                let file = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(self.path(0))?;
                self.file = Some(file);
                self.size = 0;
            } else {
                for index in (1..self.max_files).rev() {
                    let from = self.path(index - 1);
                    if from.exists() {
                        fs::rename(from, self.path(index))?;
                    }
                }
                self.open_current()?;
            }
            self.next_rotation_attempt = None;
            self.rotation_warning_reported = false;
            Ok(())
        }
    }

    impl Write for RotatingFile {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.size > 0
                && self.size.saturating_add(buf.len() as u64) > self.max_bytes
                && self
                    .next_rotation_attempt
                    .is_none_or(|at| Instant::now() >= at)
            {
                if let Err(error) = self.rotate() {
                    if !self.rotation_warning_reported {
                        eprintln!("WARN log rotation failed in {:?}: {error}", self.dir);
                        self.rotation_warning_reported = true;
                    }
                    self.next_rotation_attempt = Some(Instant::now() + Duration::from_secs(60));
                }
            }
            // The console layer already received the event. Never let a file
            // error stop logging or the node itself.
            if let Some(file) = self.file.as_mut() {
                if let Err(error) = file.write_all(buf) {
                    if !self.rotation_warning_reported {
                        eprintln!("WARN file logging disabled: {error}");
                        self.rotation_warning_reported = true;
                    }
                    self.file = None;
                    return Ok(buf.len());
                }
                self.size = self.size.saturating_add(buf.len() as u64);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.file.as_mut() {
                Some(file) => file.flush(),
                None => Ok(()),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn read(file: &RotatingFile, index: usize) -> Option<String> {
            fs::read_to_string(file.path(index)).ok()
        }

        #[cfg(unix)]
        #[test]
        fn read_only_log_dir_is_unusable() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
            let result = RotatingFile::open(dir.path().into(), "mina.log", 10, 2);
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            assert!(result.is_err());
        }

        #[test]
        fn missing_parent_is_created() {
            let dir = tempfile::tempdir().unwrap();
            let nested = dir.path().join("new").join("logs");
            RotatingFile::open(nested.clone(), "mina.log", 10, 2).unwrap();
            assert!(nested.join("mina.log").exists());
        }

        #[test]
        fn file_as_log_directory_is_unusable() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("not-a-directory");
            fs::write(&path, "content").unwrap();
            assert!(RotatingFile::open(path, "mina.log", 10, 2).is_err());
        }

        #[test]
        fn rotation_failure_keeps_current_file_and_backs_off() {
            let dir = tempfile::tempdir().unwrap();
            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 4, 2).unwrap();
            file.write_all(b"1234").unwrap();
            fs::create_dir(dir.path().join("mina.log.1")).unwrap();
            file.write_all(b"5").unwrap();
            let retry_at = file.next_rotation_attempt.unwrap();
            file.write_all(b"6").unwrap();
            assert_eq!(file.next_rotation_attempt, Some(retry_at));
            assert_eq!(read(&file, 0).as_deref(), Some("123456"));
        }

        #[test]
        fn rotates_by_size_and_keeps_a_bounded_number_of_files() {
            let dir = tempfile::tempdir().unwrap();
            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 10, 3).unwrap();
            for line in [
                "aaaa\n", "bbbb\n", "cccc\n", "dddd\n", "eeee\n", "ffff\n", "gggg\n",
            ] {
                file.write_all(line.as_bytes()).unwrap();
            }
            file.flush().unwrap();

            // Two 5-byte lines fill a 10-byte file; the third rotates it.
            assert_eq!(read(&file, 0).as_deref(), Some("gggg\n"));
            assert_eq!(read(&file, 1).as_deref(), Some("eeee\nffff\n"));
            assert_eq!(read(&file, 2).as_deref(), Some("cccc\ndddd\n"));
            assert_eq!(read(&file, 3), None, "the oldest file is deleted");
            let names = fs::read_dir(dir.path()).unwrap().count();
            assert_eq!(names, 3);
        }

        #[test]
        fn a_write_is_never_split_between_files() {
            let dir = tempfile::tempdir().unwrap();
            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 8, 2).unwrap();
            file.write_all(b"12345\n").unwrap();
            // Larger than the bound: rotates first, then goes whole into the
            // new file.
            file.write_all(b"a long line of 23 bytes\n").unwrap();
            file.write_all(b"x\n").unwrap();
            assert_eq!(read(&file, 1).as_deref(), Some("a long line of 23 bytes\n"));
            assert_eq!(read(&file, 0).as_deref(), Some("x\n"));
            assert_eq!(read(&file, 2), None);
        }

        #[test]
        fn reopening_appends_and_counts_the_existing_size() {
            let dir = tempfile::tempdir().unwrap();
            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 10, 2).unwrap();
            file.write_all(b"abcdefgh\n").unwrap();
            drop(file);

            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 10, 2).unwrap();
            file.write_all(b"ij\n").unwrap();
            assert_eq!(read(&file, 1).as_deref(), Some("abcdefgh\n"));
            assert_eq!(read(&file, 0).as_deref(), Some("ij\n"));
        }

        #[test]
        fn a_smaller_bound_deletes_the_files_beyond_it() {
            let dir = tempfile::tempdir().unwrap();
            for name in [
                "mina.log.1",
                "mina.log.2",
                "mina.log.3",
                "mina.log.2026-10-22",
            ] {
                fs::write(dir.path().join(name), "old\n").unwrap();
            }
            let file = RotatingFile::open(dir.path().into(), "mina.log", 10, 2).unwrap();
            assert_eq!(read(&file, 1).as_deref(), Some("old\n"));
            assert_eq!(read(&file, 2), None);
            assert_eq!(read(&file, 3), None);
            // The daily files of older builds aren't this writer's to delete.
            assert!(dir.path().join("mina.log.2026-10-22").exists());
        }

        #[test]
        fn one_file_is_truncated_instead_of_rotated() {
            let dir = tempfile::tempdir().unwrap();
            let mut file = RotatingFile::open(dir.path().into(), "mina.log", 6, 1).unwrap();
            file.write_all(b"1234\n").unwrap();
            file.write_all(b"5678\n").unwrap();
            assert_eq!(read(&file, 0).as_deref(), Some("5678\n"));
            assert_eq!(read(&file, 1), None);
        }
    }
}

#[cfg(target_family = "wasm")]
mod web {
    use super::*;
    use tracing_wasm::{set_as_global_default_with_config, WASMLayerConfigBuilder};

    pub fn initialize(max_log_level: Level) {
        let mut config = WASMLayerConfigBuilder::new();
        config.set_max_level(max_log_level);
        set_as_global_default_with_config(config.build());
    }
}

#[cfg(not(target_family = "wasm"))]
pub use native::{initialize, initialize_with_filesystem_output, LogFileOptions, RotatingFile};
#[cfg(target_family = "wasm")]
pub use web::initialize;
