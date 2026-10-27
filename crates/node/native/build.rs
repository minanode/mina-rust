//! Rebuilds the crate when the embedded dashboard (`ui/dashboard/dist`,
//! F-console) changes, appears or goes. The `dashboard` feature embeds it
//! with `rust-embed`, whose files Cargo already tracks once embedded; a dist
//! built after the crate was compiled without one needs this.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_DASHBOARD").is_none() {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let ui = manifest.join("../../../ui/dashboard");
    let dist = ui.join("dist");
    println!(
        "cargo:rerun-if-changed={}",
        ui.join("src/routes.json").display()
    );
    if dist.join("index.html").is_file() {
        println!("cargo:rerun-if-changed={}", dist.display());
    } else {
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            println!(
                "cargo:warning=dashboard not built: run `cd ui && npm ci && npm run build:dashboard`"
            );
        }
        // Watch for dist to appear: a missing path would rerun every build.
        println!("cargo:rerun-if-changed={}", ui.display());
    }
}
