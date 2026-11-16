fn main() -> Result<(), String> {
    println!("cargo::rerun-if-changed=build.rs");
    let ffmpeg = std::env::var("DEP_NGINE_RECORDER_FFMPEG_LIB_DIR")
        .map_err(|_| "ngine-recorder did not say where the FFmpeg libraries are")?;
    let family = std::env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    if family.split(',').any(|family| family == "unix") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,{ffmpeg}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    Ok(())
}
