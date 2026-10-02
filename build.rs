use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/events/pipewire_dlopen.c");
    println!("cargo:rerun-if-changed=src/events/dbus_sdbus.c");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj_path = out_dir.join("pipewire_dlopen.o");
    let dbus_obj_path = out_dir.join("dbus_sdbus.o");
    let lib_path = out_dir.join("libpipewire-0.3.a");

    let status = Command::new("gcc")
        .args([
            "-c",
            "-O2",
            "-fPIC",
            "src/events/pipewire_dlopen.c",
            "-o",
        ])
        .arg(&obj_path)
        .status()
        .expect("failed to compile pipewire_dlopen.c");
    assert!(status.success(), "compilation of pipewire_dlopen.c failed");

    let status = Command::new("gcc")
        .args([
            "-c",
            "-O2",
            "-fPIC",
            "src/events/dbus_sdbus.c",
            "-o",
        ])
        .arg(&dbus_obj_path)
        .status()
        .expect("failed to compile dbus_sdbus.c");
    assert!(status.success(), "compilation of dbus_sdbus.c failed");

    let status = Command::new("ar")
        .args(["rcs"])
        .arg(&lib_path)
        .arg(&obj_path)
        .arg(&dbus_obj_path)
        .status()
        .expect("failed to create libpipewire-0.3.a");
    assert!(status.success(), "ar failed to create libpipewire-0.3.a");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=pipewire-0.3");
    println!("cargo:rustc-link-lib=systemd");
    println!("cargo:rustc-link-arg=-Wl,--as-needed");

    for tray_src in [
        "examples/capsules/components/tray/cellbar_tray.c",
        "examples/flat/components/tray/cellbar_tray.c",
    ] {
        println!("cargo:rerun-if-changed={tray_src}");
        if std::path::Path::new(tray_src).exists() {
            let out_bin = std::path::Path::new(tray_src).with_file_name("cellbar-tray");
            let status = Command::new("gcc")
                .args([
                    "-O3",
                    "-Wall",
                    "-Wno-format-truncation",
                    tray_src,
                    "-o",
                    out_bin.to_str().unwrap(),
                    "-lsystemd",
                    "-lz",
                ])
                .status();
            match status {
                Ok(status) if status.success() => {
                    println!("cargo:warning=built {}", out_bin.display());
                }
                Ok(status) => {
                    println!(
                        "cargo:warning=cellbar-tray was not built (gcc exited with {status}); tray is optional"
                    );
                }
                Err(error) => {
                    println!(
                        "cargo:warning=cellbar-tray was not built ({error}); install gcc, systemd, and zlib development libraries to enable tray"
                    );
                }
            }
        }
    }
}
