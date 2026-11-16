use std::env;
use std::path::{Path, PathBuf};

const LIBRARIES: [&str; 5] = ["avformat", "avcodec", "swscale", "swresample", "avutil"];

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed=NGINE_FFMPEG_DIR");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    let target = required_env("TARGET")?;
    let root = ffmpeg_root(&target)?;
    let include = root.join("include");
    let lib = root.join("lib");
    println!("cargo:rerun-if-changed={}", include.display());

    println!("cargo:rustc-link-search=native={}", lib.display());
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("bin").display()
    );
    for library in LIBRARIES {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    if required_env("CARGO_CFG_TARGET_FAMILY")?
        .split(',')
        .any(|family| family == "unix")
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    }
    println!("cargo:lib-dir={}", lib.display());

    let out = PathBuf::from(required_env("OUT_DIR")?).join("bindings.rs");
    bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("(av|avcodec|avformat|avio|sws|swr)_.*")
        .allowlist_var("(AV|AVERROR|AVFMT|AVIO|AVSEEK|SWS|SWR|FF|LIBAV|NGINE)_.*")
        .allowlist_type("AVHW(Device|Frames)Context")
        .blocklist_function(".*_v[a-z]*printf|av_log_.*callback|av_vlog|av_vbprintf")
        .default_enum_style(bindgen::EnumVariation::NewType {
            is_bitfield: false,
            is_global: true,
        })
        .prepend_enum_name(false)
        .derive_debug(false)
        .generate_comments(false)
        .generate()
        .map_err(|error| {
            format!(
                "bindgen could not read the FFmpeg headers in {}: {error}",
                include.display()
            )
        })?
        .write_to_file(&out)
        .map_err(|error| format!("could not write {}: {error}", out.display()))
}

fn ffmpeg_root(target: &str) -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("NGINE_FFMPEG_DIR") {
        let dir = PathBuf::from(dir);
        return if has_headers(&dir) {
            Ok(dir)
        } else {
            Err(format!(
                "NGINE_FFMPEG_DIR is {}, which has no include/libavcodec/avcodec.h. {}",
                dir.display(),
                how_to_build(target)
            ))
        };
    }
    let manifest = PathBuf::from(required_env("CARGO_MANIFEST_DIR")?);
    let workspace = manifest
        .ancestors()
        .nth(2)
        .ok_or("ngine-media must sit two directories below the workspace root")?;
    let dir = workspace.join("target").join("ffmpeg").join(target);
    if has_headers(&dir) {
        Ok(dir)
    } else {
        Err(format!(
            "FFmpeg was not found in {}. {}",
            dir.display(),
            how_to_build(target)
        ))
    }
}

fn has_headers(dir: &Path) -> bool {
    dir.join("include/libavcodec/avcodec.h").is_file()
}

fn how_to_build(target: &str) -> String {
    format!(
        "Run ffmpeg/build.sh from the workspace root, which writes target/ffmpeg/{target}/, \
         or set NGINE_FFMPEG_DIR to a directory that holds include/ and lib/ of an FFmpeg 9.0 LGPL shared build."
    )
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("cargo did not set {name}"))
}
