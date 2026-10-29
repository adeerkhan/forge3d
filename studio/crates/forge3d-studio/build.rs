//! Links libpython into the Studio executable, and makes GPUI's X11/wayland
//! libraries resolvable on hosts that ship only the versioned runtime objects.
//!
//! forge3d is consumed with its `extension-module` feature enabled (see
//! studio/Cargo.toml for why). Under that feature pyo3 does not link
//! libpython — wheels get the symbols from the host interpreter. Studio is a
//! standalone binary, so we resolve libpython through pyo3's own build
//! configuration and embed it, exactly like a Python-embedding build.
//! The binary never initializes the interpreter.
//!
//! The library is appended as a trailing link argument (with
//! `--no-as-needed`) because rustc emits `rustc-link-lib` entries *before*
//! the object files, where rustc's default `--as-needed` drops libpython
//! before any forge3d symbols reference it.
//!
//! `libxkbcommon`: GPUI links `-lxkbcommon` and `-lxkbcommon-x11`, which needs
//! the unversioned development symlinks. Fedora/RHEL ship only
//! `libxkbcommon.so.0` unless `libxkbcommon-devel` is installed, and this
//! build must not require a root install. When the development symlink is
//! missing we create one in `OUT_DIR` pointing at the runtime object and add
//! that directory to the link search path, so `cargo run` works without
//! `RUSTFLAGS`. If the runtime object is missing too, the build says so
//! instead of failing deep inside `cc`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        provision_missing_dev_symlinks();
    }

    let config = pyo3_build_config::get();
    let Some(lib_name) = &config.lib_name else {
        panic!(
            "pyo3 build config has no libpython name; set PYO3_PYTHON to a Python \
             installation with development files (see studio/README.md)"
        );
    };
    let Some(lib_dir) = &config.lib_dir else {
        panic!(
            "pyo3 build config has no libpython directory; set PYO3_PYTHON to a Python \
             installation with development files (see studio/README.md)"
        );
    };

    if target_os == "linux" {
        println!("cargo:rustc-link-search=native={lib_dir}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
        println!("cargo:rustc-link-arg=-Wl,--no-as-needed,-l{lib_name}");
    } else {
        // Other platforms: pyo3's default dynamic lookup applies; revisit when
        // packaging for Windows/macOS per studio/README.md.
        println!("cargo:rustc-link-arg=-L{lib_dir}");
        println!("cargo:rustc-link-arg=-l{lib_name}");
    }
}

/// gpui-pre-linux links these by bare name; make sure the linker can find them.
const LAYOUT_LIBS: &[&str] = &["libxkbcommon", "libxkbcommon-x11"];

fn provision_missing_dev_symlinks() {
    let search_dirs: Vec<PathBuf> = [
        "/usr/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib",
        "/lib64",
        "/lib/x86_64-linux-gnu",
        "/lib",
    ]
    .iter()
    .map(PathBuf::from)
    .filter(|dir| dir.is_dir())
    .collect();

    let mut missing: Vec<&str> = Vec::new();
    for lib in LAYOUT_LIBS {
        // A development symlink already on the search path: nothing to do.
        let has_dev = search_dirs.iter().any(|dir| dir.join(format!("{lib}.so")).exists());
        if has_dev {
            continue;
        }
        let Some(runtime) = search_dirs
            .iter()
            .flat_map(|dir| std::fs::read_dir(dir).ok().into_iter().flatten())
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                name.starts_with(&format!("{lib}.so.")) && path.is_file()
            })
            .max()
        else {
            missing.push(lib);
            continue;
        };

        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let compat = out_dir.join("xkb-compat");
        if std::fs::create_dir_all(&compat).is_err() {
            missing.push(lib);
            continue;
        }
        let link = compat.join(format!("{lib}.so"));
        if !link.exists() {
            if let Err(error) = std::os::unix::fs::symlink(&runtime, &link) {
                println!(
                    "cargo:warning=could not link {} at {}: {error}",
                    link.display(),
                    runtime.display()
                );
                missing.push(lib);
                continue;
            }
        }
        println!("cargo:rustc-link-search=native={}", compat.display());
        // Belt and braces: the final binary link also gets the directory
        // directly, which is where the `-lxkbcommon` flags are consumed.
        println!("cargo:rustc-link-arg=-L{}", compat.display());
        println!(
            "cargo:warning={lib} has no development symlink; linking {} instead (install {}-devel to remove this)",
            runtime.display(),
            lib
        );
    }

    for lib in missing {
        println!(
            "cargo:warning={lib} runtime library not found; GPUI needs it. Install {lib} (and -devel) with your package manager."
        );
    }
}
