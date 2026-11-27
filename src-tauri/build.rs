fn main() {
    println!("cargo:rustc-check-cfg=cfg(gall_wayland_presentation)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        build_wayland_presentation();
    }
    println!("cargo:rerun-if-env-changed=GALL_UPDATER_PUBLIC_KEY");
    // Git resolves per-worktree HEAD and shared ref storage for linked worktrees.
    for name in ["HEAD", "packed-refs"] {
        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--git-path", name])
            .output()
        {
            if output.status.success() {
                println!(
                    "cargo:rerun-if-changed={}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );
            }
        }
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
    {
        if output.status.success() {
            let reference = String::from_utf8_lossy(&output.stdout);
            if let Ok(path) = std::process::Command::new("git")
                .args(["rev-parse", "--git-path", reference.trim()])
                .output()
            {
                if path.status.success() {
                    println!(
                        "cargo:rerun-if-changed={}",
                        String::from_utf8_lossy(&path.stdout).trim()
                    );
                }
            }
        }
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GALL_BUILD_REVISION={revision}");
    tauri_build::build()
}

fn build_wayland_presentation() {
    use std::{path::PathBuf, process::Command};

    let Ok(path) = pkg_config::get_variable("wayland-protocols", "pkgdatadir") else {
        return;
    };
    let protocol = PathBuf::from(path).join("stable/presentation-time/presentation-time.xml");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    let header = out.join("presentation-client.h");
    let source = out.join("presentation-protocol.c");
    for (mode, destination) in [("client-header", &header), ("private-code", &source)] {
        if !Command::new("wayland-scanner")
            .arg(mode)
            .arg(&protocol)
            .arg(destination)
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }
    }
    let Ok(gtk) = pkg_config::Config::new().probe("gdk-wayland-3.0") else {
        return;
    };
    let Ok(wayland) = pkg_config::Config::new().probe("wayland-client") else {
        return;
    };
    println!("cargo:rerun-if-changed={}", protocol.display());
    println!("cargo:rerun-if-changed=src/wayland_presentation.c");
    cc::Build::new()
        .file("src/wayland_presentation.c")
        .file(source)
        .include(out)
        .includes(gtk.include_paths)
        .includes(wayland.include_paths)
        .flag_if_supported("-Wno-unused-parameter")
        .compile("gall_wayland_presentation");
    println!("cargo:rustc-cfg=gall_wayland_presentation");
}
