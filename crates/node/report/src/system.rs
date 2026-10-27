//! The machine a report comes from: OS, kernel, CPU, memory and disk.

use std::{path::Path, process::Command};

#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    pub os: String,
    pub kernel: Option<String>,
    pub arch: &'static str,
    pub cpu_model: Option<String>,
    pub cpu_threads: usize,
    /// `None` off x86-64, where the question doesn't apply.
    pub bmi2: Option<bool>,
    pub adx: Option<bool>,
    pub ram_total_bytes: Option<u64>,
    pub ram_available_bytes: Option<u64>,
    /// Free space on the work directory's file system.
    pub disk_free_bytes: Option<u64>,
}

pub fn collect(work_dir: &Path) -> SystemInfo {
    let (bmi2, adx) = cpu_features();
    let (ram_total_bytes, ram_available_bytes) = memory();
    SystemInfo {
        os: os_name(),
        kernel: kernel(),
        arch: std::env::consts::ARCH,
        cpu_model: cpu_model(),
        cpu_threads: std::thread::available_parallelism().map_or(0, |n| n.get()),
        bmi2,
        adx,
        ram_total_bytes,
        ram_available_bytes,
        disk_free_bytes: disk_free(work_dir),
    }
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn os_name() -> String {
    let release = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                .map(|name| name.trim_matches('"').to_owned())
        });
    match (release, std::env::consts::OS) {
        (Some(name), _) => name,
        (None, "macos") => output("sw_vers", &["-productVersion"])
            .map_or("macOS".to_owned(), |v| format!("macOS {v}")),
        (None, os) => os.to_owned(),
    }
}

fn kernel() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_owned())
        .or_else(|| output("uname", &["-r"]))
}

fn cpu_model() -> Option<String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .as_deref()
        .and_then(cpu_model_of)
        .or_else(|| output("sysctl", &["-n", "machdep.cpu.brand_string"]))
}

/// The CPU in `/proc/cpuinfo`: its model name, or on ARM64, which has none
/// (RC7-FIXES: Docker on Apple silicon), its implementer and part. No tool
/// such as `lscpu` is needed: the release image has none.
fn cpu_model_of(cpuinfo: &str) -> Option<String> {
    let field = |name: &str| {
        cpuinfo.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == name)
                .then(|| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
    };
    if let Some(model) = field("model name")
        .or_else(|| field("Hardware"))
        .or_else(|| field("Model"))
    {
        return Some(model);
    }
    let implementer = field("CPU implementer")?;
    // Arm's list of implementer codes (MIDR_EL1 bits 31:24).
    let vendor = match implementer.to_ascii_lowercase().as_str() {
        "0x41" => "Arm",
        "0x42" => "Broadcom",
        "0x46" => "Fujitsu",
        "0x48" => "HiSilicon",
        "0x4e" => "NVIDIA",
        "0x51" => "Qualcomm",
        "0x61" => "Apple",
        "0xc0" => "Ampere",
        _ => "ARM",
    };
    Some(match field("CPU part") {
        Some(part) => format!("{vendor} CPU (implementer {implementer}, part {part})"),
        None => format!("{vendor} CPU (implementer {implementer})"),
    })
}

#[cfg(target_arch = "x86_64")]
fn cpu_features() -> (Option<bool>, Option<bool>) {
    (
        Some(std::arch::is_x86_feature_detected!("bmi2")),
        Some(std::arch::is_x86_feature_detected!("adx")),
    )
}

#[cfg(not(target_arch = "x86_64"))]
fn cpu_features() -> (Option<bool>, Option<bool>) {
    (None, None)
}

fn memory() -> (Option<u64>, Option<u64>) {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        let field = |name: &str| {
            text.lines().find_map(|line| {
                let kib = line.strip_prefix(name)?.trim().strip_suffix("kB")?;
                kib.trim().parse::<u64>().ok().map(|kib| kib * 1024)
            })
        };
        return (field("MemTotal:"), field("MemAvailable:"));
    }
    (
        output("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse().ok()),
        None,
    )
}

/// Free bytes on the file system of `dir`, or of its nearest existing
/// ancestor, from `statvfs` (RC7-FIXES: the release image has no `df`).
#[cfg(unix)]
fn disk_free(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let dir = dir.ancestors().find(|d| d.exists())?;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid C string and `stat` a writable statvfs.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // The space an unprivileged user may use, as `df` reports it.
    u64::try_from(stat.f_bavail)
        .ok()?
        .checked_mul(u64::try_from(stat.f_frsize).ok()?)
}

#[cfg(not(unix))]
fn disk_free(_dir: &Path) -> Option<u64> {
    None
}

/// Whether process `pid` runs, and its resident memory, on Linux.
pub fn process(pid: u32) -> Option<ProcessInfo> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let rss_bytes = status.lines().find_map(|line| {
        let kib = line.strip_prefix("VmRSS:")?.trim().strip_suffix("kB")?;
        kib.trim().parse::<u64>().ok().map(|kib| kib * 1024)
    });
    Some(ProcessInfo { rss_bytes })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessInfo {
    pub rss_bytes: Option<u64>,
}

/// `bytes` in GiB, one decimal.
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc7_the_cpu_is_named_without_lscpu() {
        let x86 = "processor\t: 0\nvendor_id\t: GenuineIntel\n\
                   model name\t: Intel(R) Xeon(R) CPU @ 2.20GHz\nflags\t\t: fpu bmi2 adx\n";
        assert_eq!(
            cpu_model_of(x86).as_deref(),
            Some("Intel(R) Xeon(R) CPU @ 2.20GHz")
        );
        // Linux on Apple silicon (Docker on a Mac) has no model name.
        let apple = "processor\t: 0\nBogoMIPS\t: 48.00\nFeatures\t: fp asimd\n\
                     CPU implementer\t: 0x61\nCPU architecture: 8\nCPU variant\t: 0x0\n\
                     CPU part\t: 0x039\nCPU revision\t: 0\n";
        assert_eq!(
            cpu_model_of(apple).as_deref(),
            Some("Apple CPU (implementer 0x61, part 0x039)")
        );
        // A Raspberry Pi 4 lists its board as the model.
        let pi = "CPU implementer\t: 0x41\nCPU part\t: 0xd08\n\
                  Model\t\t: Raspberry Pi 4 Model B Rev 1.4\n";
        assert_eq!(
            cpu_model_of(pi).as_deref(),
            Some("Raspberry Pi 4 Model B Rev 1.4")
        );
        assert_eq!(cpu_model_of("processor\t: 0\n"), None);
    }

    #[test]
    fn rc7_free_disk_is_read_without_df() {
        let dir = tempfile::tempdir().unwrap();
        let free = disk_free(&dir.path().join("not/made/yet"));
        assert!(free.is_some_and(|free| free > 0) || !cfg!(unix), "{free:?}");
    }

    #[test]
    fn this_machine_is_described() {
        let info = collect(Path::new("/nonexistent/work/dir"));
        assert!(!info.os.is_empty());
        assert!(info.cpu_threads > 0);
        assert_eq!(gib(3 << 29), "1.5 GiB");
        assert!(process(std::process::id()).is_some() || !cfg!(target_os = "linux"));
    }
}
