fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let builds_for_macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if builds_for_macos {
        println!("cargo::rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
