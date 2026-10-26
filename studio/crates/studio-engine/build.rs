//! Links libpython into the Studio executable.
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

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

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

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
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
