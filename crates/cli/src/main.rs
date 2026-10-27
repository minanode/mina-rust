// jemalloc fixes its page size at build time and rejects larger kernel pages.
// .cargo/config.toml builds it for 64 KiB pages on 64-bit ARM Linux, so one
// binary runs on 4, 16 and 64 KiB kernels.
#[cfg(not(target_family = "wasm"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// MEM-JE's 8h devnet comparisons cut full-node RSS by about 20% with CPU
// within 5% of control.
// jemalloc reads this before _RJEM_MALLOC_CONF, preserving operator overrides.
#[cfg(not(target_family = "wasm"))]
#[used]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: Option<&'static std::ffi::c_char> =
    // SAFETY: the NUL-terminated C literal has static storage. jemalloc only
    // reads it, and Option<&c_char> matches its nullable configuration pointer.
    Some(unsafe { &*c"narenas:4".as_ptr() });

pub mod commands;
use clap::Parser;

#[cfg(not(target_family = "wasm"))]
mod panic_hook;

mod exit_with_error;
pub use exit_with_error::{exit_with_error, exit_with_error_code};

#[cfg(feature = "unsafe-signal-handlers")]
mod unsafe_signal_handlers {
    use nix::libc;

    extern "C" fn handle_sigsegv(_signal: libc::c_int) {
        eprintln!("########### SIGSEGV #############");
        mina_node::recorder::Recorder::graceful_shutdown();
        std::process::exit(1);
    }

    pub fn setup() {
        let stack_t = libc::stack_t {
            ss_sp: {
                let stack = Box::<[u8; libc::SIGSTKSZ]>::new([0; libc::SIGSTKSZ]);
                Box::into_raw(stack) as *mut _
            },
            ss_flags: 0,
            ss_size: libc::SIGSTKSZ,
        };

        let res = unsafe { libc::sigaltstack(&stack_t as *const _, std::ptr::null_mut()) };
        assert_eq!(res, 0);

        let action = libc::sigaction {
            sa_sigaction: handle_sigsegv as _,
            sa_mask: unsafe { std::mem::zeroed() },
            sa_flags: libc::SA_ONSTACK,
            sa_restorer: None,
        };
        let res = unsafe { libc::sigaction(libc::SIGSEGV, &action as _, std::ptr::null_mut()) };
        assert_eq!(res, 0);
    }
}

fn setup_var_from_single_and_only_thread() {
    const VARS: &[(&str, &str)] = &[("RUST_BACKTRACE", "full")];

    for (name, value) in VARS {
        if std::env::var(name).is_err() {
            // Safe to call, we didn't launch any threads yet
            unsafe { std::env::set_var(name, value) };
        }
    }
}

fn early_setup() {
    setup_var_from_single_and_only_thread();
    #[cfg(not(target_family = "wasm"))]
    std::panic::set_hook(Box::new(panic_hook::new_hook));
}

fn main() -> anyhow::Result<()> {
    early_setup();

    #[cfg(feature = "unsafe-signal-handlers")]
    unsafe_signal_handlers::setup();
    let app = commands::MinaCli::parse();

    let network_init_result = match app.network {
        commands::Network::Devnet => mina_core::NetworkConfig::init("devnet"),
        commands::Network::Mainnet => mina_core::NetworkConfig::init("mainnet"),
    };

    network_init_result.expect("Failed to initialize network configuration");

    app.command.run(app.network)
}

#[cfg(all(test, not(target_family = "wasm")))]
mod allocator_tests {
    use std::ffi::{c_void, CStr};

    fn mallctl_read<T: Copy + Default>(name: &CStr) -> T {
        let mut value = T::default();
        let mut len = std::mem::size_of::<T>();
        // SAFETY: `name` is NUL-terminated and `value`/`len` describe a valid,
        // correctly sized output buffer; no new value is written.
        let result = unsafe {
            tikv_jemalloc_sys::mallctl(
                name.as_ptr(),
                (&mut value as *mut T).cast::<c_void>(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(result, 0, "mallctl {name:?}");
        assert_eq!(len, std::mem::size_of::<T>());
        value
    }

    /// jemalloc aborts at startup when the kernel page is larger than its
    /// compiled page, so 64-bit ARM Linux must use the 64 KiB build.
    #[test]
    fn jemalloc_page_size_covers_target_kernels() {
        let page: usize = mallctl_read(c"arenas.page");
        // SAFETY: sysconf has no preconditions.
        let os_page = unsafe { nix::libc::sysconf(nix::libc::_SC_PAGESIZE) } as usize;
        assert!(
            page >= os_page,
            "jemalloc page {page} < kernel page {os_page}"
        );
        if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
            assert_eq!(page, 65536);
        }
    }

    /// The compiled `narenas:4` default applies unless an operator overrides it.
    #[test]
    fn jemalloc_uses_compiled_arena_default() {
        if std::env::var_os("_RJEM_MALLOC_CONF").is_some() {
            return;
        }
        let narenas: u32 = mallctl_read(c"opt.narenas");
        assert_eq!(narenas, 4);
    }
}
