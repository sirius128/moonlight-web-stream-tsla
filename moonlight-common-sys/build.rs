use std::env::var;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(moonlight_audio_threshold_patched)");

    #[cfg(feature = "generate-bindings")]
    generate_bindings();

    let allow_vendored = var("MOONLIGHT_COMMON_NO_VENDOR").is_err();

    #[allow(unused)]
    let moonlight_output: Option<(String, PathBuf)> = None;

    #[cfg(feature = "vendored")]
    let moonlight_output = compile_moonlight(allow_vendored);

    link(moonlight_output, allow_vendored);
}

#[cfg(feature = "generate-bindings")]
fn generate_bindings() {
    generate_bindings_with_name("limelight.h", "limelight.rs");
    #[cfg(feature = "crypto")]
    generate_bindings_with_name("crypto.h", "crypto.rs");
}
#[cfg(feature = "generate-bindings")]
fn generate_bindings_with_name(header_name: &str, rust_name: &str) {
    let bindings = bindgen::Builder::default()
        .header(header_name)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join(rust_name))
        .expect("Couldn't write bindings!");
}

#[cfg(feature = "vendored")]
fn compile_moonlight(allow_vendored: bool) -> Option<(String, PathBuf)> {
    if !allow_vendored {
        return None;
    }

    // Rerun when changed
    println!("cargo::rerun-if-changed=moonlight-common-c");
    println!("cargo::rerun-if-env-changed={AUDIO_THRESHOLD_ENV}");
    println!("cargo::rerun-if-env-changed=DEP_OPENSSL_ROOT");
    println!("cargo::rerun-if-env-changed=OPENSSL_ROOT_DIR");
    println!("cargo::rerun-if-env-changed=DEP_OPENSSL_INCLUDE");
    println!("cargo::rerun-if-env-changed=OPENSSL_INCLUDE");
    println!("cargo::rerun-if-env-changed=OPENSSL_INCLUDE_DIR");
    println!("cargo::rerun-if-env-changed=DEP_OPENSSL_LIB");
    println!("cargo::rerun-if-env-changed=OPENSSL_LIB_DIR");
    println!("cargo::rerun-if-env-changed=OPENSSL_CRYPTO_LIBRARY");

    let source_dir = patched_moonlight_source();
    println!("cargo::rustc-cfg=moonlight_audio_threshold_patched");

    // builds into $OUT_DIR
    let mut config = cmake::Config::new(&source_dir);
    config.define("BUILD_SHARED_LIBS", "OFF");
    config.define("CMAKE_TRY_COMPILE_TARGET_TYPE", "STATIC_LIBRARY");

    // -- Link OpenSSL: Some environment variables are exported from openssl-sys for all dependents
    if let Ok(ssl_root) = var("DEP_OPENSSL_ROOT").or_else(|_| var("OPENSSL_ROOT_DIR")) {
        config.define("OPENSSL_ROOT_DIR", ssl_root);
    }

    // Include
    let ssl_include = var("DEP_OPENSSL_INCLUDE")
        .or(var("OPENSSL_INCLUDE"))
        .unwrap_or_else(|_| {
            let mut ssl_root = var("DEP_OPENSSL_ROOT").expect("failed to find openssl");
            ssl_root.push_str("/include");

            ssl_root
        });
    config.define("OPENSSL_INCLUDE_DIR", ssl_include);

    // Lib
    let ssl_libs = var("DEP_OPENSSL_LIB")
        .or(var("OPENSSL_LIB_DIR"))
        .unwrap_or_else(|_| {
            let mut ssl_root = var("DEP_OPENSSL_ROOT").expect("failed to find openssl");
            ssl_root.push_str("/lib");

            ssl_root
        });
    let lib_ext = {
        let target_os = var("CARGO_CFG_TARGET_OS").unwrap();
        let target_env = var("CARGO_CFG_TARGET_ENV").unwrap();

        match (target_os.as_str(), target_env.as_str()) {
            ("windows", "msvc") => "lib",
            ("windows", "gnu") => "a",
            // other OSes
            (_, _) => "a",
        }
    };

    let crypto_lib = format!("{ssl_libs}/libcrypto.{lib_ext}");
    config.define("OPENSSL_CRYPTO_LIBRARY", &crypto_lib);

    // Force the library used by openssl
    config.define("OPENSSL_USE_STATIC_LIBS", "TRUE");

    // If we're in cargo cross
    if let Ok("1") = var("MOONLIGHT_COMMON_CROSS").as_deref() {
        let target_os = var("CARGO_CFG_TARGET_OS").unwrap();

        if target_os == "windows" {
            // Definitions required for some windows headers to enable them
            // -> qos2.h
            let flags = "-D_WIN32_WINNT=0x0600 -DHAS_PQOS_FLOWID -DHAS_QOS_FLOWID";
            config.cflag(flags);
            config.cxxflag(flags);
        }
    }

    let profile = config.get_profile().to_string();
    Some((profile, config.build()))
}

// Sunshine picks 512 kbps stereo Opus only when the RTSP Host it receives is not
// 0.0.0.0, and moonlight-common-c sends 0.0.0.0 whenever the video bitrate is
// below HIGH_AUDIO_BITRATE_THRESHOLD (15 Mbps). The Tesla browser can't decode
// video anywhere near that, so the threshold is turned into a runtime variable
// (default from AUDIO_THRESHOLD_ENV, 0 = always high quality) that the stream
// settings can override per session. The submodule itself is left untouched;
// a patched copy is built from OUT_DIR instead.
#[cfg(feature = "vendored")]
const AUDIO_THRESHOLD_ENV: &str = "MOONLIGHT_HIGH_AUDIO_BITRATE_THRESHOLD";
#[cfg(feature = "vendored")]
const AUDIO_THRESHOLD_DEFINE: &str = "#define HIGH_AUDIO_BITRATE_THRESHOLD 15000";
#[cfg(feature = "vendored")]
const AUDIO_THRESHOLD_VAR: &str = "MlHighAudioBitrateThreshold";

#[cfg(feature = "vendored")]
fn patched_moonlight_source() -> PathBuf {
    use std::fs;

    let default_threshold: i32 = match var(AUDIO_THRESHOLD_ENV) {
        Ok(value) => value.trim().parse().unwrap_or_else(|_| {
            panic!("{AUDIO_THRESHOLD_ENV} must be an integer (kbps), got {value:?}")
        }),
        Err(_) => 0,
    };

    let out_dir = PathBuf::from(var("OUT_DIR").expect("OUT_DIR"));
    let dest = out_dir.join("moonlight-common-c-patched");
    if dest.exists() {
        fs::remove_dir_all(&dest).expect("failed to clear patched moonlight-common-c copy");
    }
    copy_dir_recursive("moonlight-common-c".as_ref(), &dest);

    let header = dest.join("src").join("Limelight-internal.h");
    let replacement = format!(
        "extern int {AUDIO_THRESHOLD_VAR};\n#define HIGH_AUDIO_BITRATE_THRESHOLD {AUDIO_THRESHOLD_VAR}"
    );
    replace_line(&header, AUDIO_THRESHOLD_DEFINE, &replacement);

    let rtsp = dest.join("src").join("RtspConnection.c");
    let mut rtsp_source = fs::read_to_string(&rtsp)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", rtsp.display()));
    if !rtsp_source.contains("HIGH_AUDIO_BITRATE_THRESHOLD") {
        panic!(
            "{} no longer references HIGH_AUDIO_BITRATE_THRESHOLD; the high quality audio patch needs updating",
            rtsp.display()
        );
    }
    rtsp_source.push_str(&format!("\nint {AUDIO_THRESHOLD_VAR} = {default_threshold};\n"));
    fs::write(&rtsp, rtsp_source)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", rtsp.display()));

    dest
}

#[cfg(feature = "vendored")]
fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) {
    use std::fs;

    fs::create_dir_all(dest)
        .unwrap_or_else(|err| panic!("failed to create {}: {err}", dest.display()));
    let entries = fs::read_dir(src)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", src.display()));
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        if entry.file_name() == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type().expect("failed to read file type");
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            // fs::copy follows symlinks, so a symlinked file is copied as its target.
            fs::copy(&from, &to).unwrap_or_else(|err| {
                panic!("failed to copy {} -> {}: {err}", from.display(), to.display())
            });
        }
    }
}

/// Replaces the line equal to `target` (ignoring trailing whitespace, so CRLF
/// and LF checkouts both match). Panics unless exactly one line matches, so an
/// upstream change can't silently disable the patch.
#[cfg(feature = "vendored")]
fn replace_line(path: &std::path::Path, target: &str, replacement: &str) {
    use std::fs;

    let source = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let mut matches = 0;
    let mut patched = String::with_capacity(source.len() + replacement.len());
    for line in source.split_inclusive('\n') {
        let content = line.trim_end();
        if content == target {
            matches += 1;
            patched.push_str(replacement);
            patched.push_str(&line[content.len()..]);
        } else {
            patched.push_str(line);
        }
    }
    if matches != 1 {
        panic!(
            "expected exactly one line `{target}` in {}, found {matches}; the high quality audio patch needs updating",
            path.display()
        );
    }
    fs::write(path, patched)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", path.display()));
}

fn link(compile_info: Option<(String, PathBuf)>, allow_vendored: bool) {
    let lib_path = var("MOONLIGHT_COMMON_LIB").ok();

    if lib_path.is_none() && compile_info.is_none() {
        panic!(
            "Failed to compile moonlight sys because a library couldn't be found. You can try:\n1. Enable the vendored flags\n2. Provide a moonlight library with the MOONLIGHT_COMMON_LIB environment variable"
        );
    }
    if !allow_vendored && lib_path.is_none() {
        panic!(
            "If you don't allow moonlight vendored you must provide a moonlight library with the MOONLIGHT_COMMON_LIB environment variable"
        );
    }

    // Enet
    if let Some((profile, path)) = &compile_info
        && allow_vendored
    {
        println!(
            "cargo:rustc-link-search=native={}/build/enet/{profile}",
            path.display()
        );
        println!(
            "cargo:rustc-link-search=native={}/build/enet",
            path.display()
        );
    } else if let Some(lib_path) = lib_path.as_ref() {
        println!("cargo:rustc-link-search=native={}/enet", lib_path);
    }
    println!("cargo:rustc-link-lib=static=enet");

    // Moonlight
    if let Some((profile, path)) = &compile_info
        && allow_vendored
    {
        println!(
            "cargo:rustc-link-search=native={}/build/{profile}",
            path.display(),
        );
        println!("cargo:rustc-link-search=native={}/build", path.display(),);
    } else if let Some(lib_path) = lib_path.as_ref() {
        println!("cargo:rustc-link-search=native={}", lib_path);
    }
    println!("cargo:rustc-link-lib=static=moonlight-common-c");

    // Windows Debug: msvcrtd.lib
    let target_os = var("CARGO_CFG_TARGET_OS").unwrap();
    let is_debug = compile_info
        .as_ref()
        .map(|(profile, _)| profile)
        .is_some_and(|profile| profile == "Debug");

    if target_os == "windows" && is_debug {
        println!("cargo:rustc-link-lib=dylib=msvcrtd");
    }
}
