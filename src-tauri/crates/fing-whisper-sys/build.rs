use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const SOURCE_DIR: &str = "whisper.cpp";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest_dir.join(SOURCE_DIR);
    if !source.join("include").join("whisper.h").exists() {
        panic!(
            "whisper.cpp sources missing at {}; run `git submodule update --init --recursive`",
            source.display()
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    for path in [
        "build.rs",
        "whisper.cpp/include",
        "whisper.cpp/src",
        "whisper.cpp/ggml/include",
        "whisper.cpp/ggml/src",
        "whisper.cpp/CMakeLists.txt",
        "whisper.cpp/ggml/CMakeLists.txt",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    println!(
        "cargo:rustc-env=FING_WHISPER_CPP_VERSION={}",
        whisper_cpp_version(&source)
    );

    build_native(&source, &target_os);
    generate_bindings(&source);
}

fn build_native(source: &Path, target_os: &str) {
    let mut config = cmake::Config::new(source);
    // Always Release, also for cargo dev builds: a debug ggml is unusably slow,
    // and RelWithDebInfo trips the MSVC static CRT config (see whisper-rs-sys).
    config
        .profile("Release")
        .define("CMAKE_BUILD_TYPE", "Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("WHISPER_BUILD_IS_DEV", "OFF")
        .define("WHISPER_BUILD_TESTS", "OFF")
        .define("WHISPER_BUILD_EXAMPLES", "OFF")
        .define("WHISPER_BUILD_SERVER", "OFF")
        .define("WHISPER_CURL", "OFF")
        .define("WHISPER_SDL2", "OFF")
        .define("WHISPER_COREML", "OFF")
        .define("WHISPER_ALL_WARNINGS", "OFF")
        .define("WHISPER_ALL_WARNINGS_3RD_PARTY", "OFF")
        .define("GGML_NATIVE", "OFF")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_CCACHE", "OFF");

    if target_os == "macos" {
        config
            .define("GGML_METAL", "ON")
            .define("GGML_METAL_NDEBUG", "ON")
            .define("GGML_METAL_EMBED_LIBRARY", "ON")
            .define("GGML_BLAS", "ON")
            .define("GGML_BLAS_VENDOR", "Apple")
            .define("GGML_ACCELERATE", "ON");
    }

    let destination = config.build();

    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("lib").display()
    );

    for lib in ["whisper", "parakeet", "ggml", "ggml-base", "ggml-cpu"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    if target_os == "macos" {
        println!("cargo:rustc-link-lib=static=ggml-metal");
        println!("cargo:rustc-link-lib=static=ggml-blas");
        for framework in ["Metal", "MetalKit", "Foundation", "Accelerate"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if target_os == "linux" {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

fn generate_bindings(source: &Path) {
    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    let header = out.join("fing_whisper_wrapper.h");
    fs::write(&header, "#include \"whisper.h\"\n#include \"parakeet.h\"\n")
        .expect("write wrapper header");

    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", source.join("include").display()))
        .clang_arg(format!(
            "-I{}",
            source.join("ggml").join("include").display()
        ))
        .allowlist_function("whisper_.*")
        .allowlist_type("whisper_.*")
        .allowlist_var("WHISPER_.*")
        .allowlist_function("parakeet_.*")
        .allowlist_type("parakeet_.*")
        .allowlist_var("PARAKEET_.*")
        .allowlist_function("ggml_backend_.*")
        .allowlist_type("ggml_backend_.*")
        .allowlist_type("ggml_log_level")
        .allowlist_type("ggml_log_callback")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("generate whisper.cpp bindings");

    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("write bindings");
}

fn whisper_cpp_version(source: &Path) -> String {
    let cmake = fs::read_to_string(source.join("CMakeLists.txt")).unwrap_or_default();
    let part = |name: &str| {
        cmake
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix(&format!("set({name} "))
                    .and_then(|rest| rest.strip_suffix(')'))
                    .map(str::trim)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "0".to_string())
    };
    format!(
        "{}.{}.{}",
        part("WHISPER_VERSION_MAJOR"),
        part("WHISPER_VERSION_MINOR"),
        part("WHISPER_VERSION_PATCH")
    )
}
