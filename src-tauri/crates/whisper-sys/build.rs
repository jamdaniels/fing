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

    let target = Target::from_env();

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
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");

    println!(
        "cargo:rustc-env=FING_WHISPER_CPP_VERSION={}",
        whisper_cpp_version(&source)
    );

    build_native(&source, &target);
    generate_bindings(&source);
}

/// The cargo target this build script compiles for (not the host: the
/// universal macOS build compiles the crate once per architecture).
struct Target {
    os: String,
    arch: String,
    msvc: bool,
    static_crt: bool,
}

impl Target {
    fn from_env() -> Self {
        let var = |name: &str| env::var(name).unwrap_or_default();
        Self {
            os: var("CARGO_CFG_TARGET_OS"),
            arch: var("CARGO_CFG_TARGET_ARCH"),
            msvc: var("CARGO_CFG_TARGET_ENV") == "msvc",
            static_crt: var("CARGO_CFG_TARGET_FEATURE")
                .split(',')
                .any(|feature| feature == "crt-static"),
        }
    }
}

fn build_native(source: &Path, target: &Target) {
    let mut config = cmake::Config::new(source);
    // Always Release, also for cargo dev builds: a debug ggml is unusably slow,
    // and RelWithDebInfo trips the MSVC static CRT config.
    config
        .profile("Release")
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

    // GGML_NATIVE is off, so the x86 instruction sets are chosen here instead
    // of by the build machine's CPU (a CI runner may have AVX-512).
    if target.arch == "x86_64" {
        // macOS 11 still runs on the Ivy Bridge Mac Pro (2013): AVX, no AVX2.
        // On Windows, Haswell (2013) and newer.
        let avx2 = if target.os == "macos" { "OFF" } else { "ON" };
        config
            .define("GGML_SSE42", "ON")
            .define("GGML_AVX", "ON")
            .define("GGML_F16C", "ON")
            .define("GGML_AVX2", avx2)
            .define("GGML_FMA", avx2)
            .define("GGML_BMI2", avx2)
            .define("GGML_AVX512", "OFF");
    }

    match target.os.as_str() {
        "macos" => {
            // Metal only on Apple Silicon; Intel Macs run on the CPU with
            // Accelerate.
            let metal = if target.arch == "aarch64" {
                "ON"
            } else {
                "OFF"
            };
            config
                .define("GGML_METAL", metal)
                .define("GGML_METAL_NDEBUG", "ON")
                .define("GGML_METAL_EMBED_LIBRARY", "ON")
                .define("GGML_BLAS", "ON")
                .define("GGML_BLAS_VENDOR", "Apple")
                .define("GGML_ACCELERATE", "ON");
        }
        "windows" => {
            config.define("GGML_VULKAN", "ON");
        }
        _ => {}
    }

    if target.msvc {
        config.cflag("/utf-8").cxxflag("/utf-8");
        // Match the static CRT that .cargo/config.toml selects for Fing.exe
        // (/MT), whichever way CMake picks the runtime library.
        config.static_crt(target.static_crt);
        config.define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW").define(
            "CMAKE_MSVC_RUNTIME_LIBRARY",
            if target.static_crt {
                "MultiThreaded"
            } else {
                "MultiThreadedDLL"
            },
        );
    }

    let destination = config.build();

    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("lib").display()
    );

    for lib in ["whisper", "parakeet", "ggml", "ggml-base", "ggml-cpu"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    match target.os.as_str() {
        "macos" => {
            if target.arch == "aarch64" {
                println!("cargo:rustc-link-lib=static=ggml-metal");
                for framework in ["Metal", "MetalKit"] {
                    println!("cargo:rustc-link-lib=framework={framework}");
                }
            }
            println!("cargo:rustc-link-lib=static=ggml-blas");
            for framework in ["Foundation", "Accelerate"] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
            println!("cargo:rustc-link-lib=dylib=c++");
        }
        "windows" => {
            println!("cargo:rustc-link-lib=static=ggml-vulkan");
            // vulkan-1.dll ships with the GPU driver; the import library
            // comes from the Vulkan SDK.
            match env::var("VULKAN_SDK") {
                Ok(sdk) => println!(
                    "cargo:rustc-link-search=native={}",
                    Path::new(&sdk).join("Lib").display()
                ),
                Err(_) => panic!("VULKAN_SDK is not set; install the Vulkan SDK"),
            }
            println!("cargo:rustc-link-lib=dylib=vulkan-1");
            println!("cargo:rustc-link-lib=dylib=advapi32");
        }
        "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        _ => {}
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
