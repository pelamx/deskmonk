//! Mac App Store build only: compiles the StoreKit bridge (packaging/appstore/Store.swift) and links it.
fn main() {
    // The icon deskmonk.exe itself shows: in Explorer, on the Desktop, on a pinned shortcut and in the
    // taskbar. Without it Windows draws a blank page, whatever icon the running window sets.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("embed the Windows app icon");
    }
    println!("cargo:rerun-if-changed=packaging/appstore/Store.swift");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    if std::env::var_os("CARGO_FEATURE_APPSTORE").is_none() || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        other => other,
    }
    .to_string();
    let lib = out.join("libdmstore.a");
    let status = std::process::Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-O", "-swift-version", "5"])
        .args(["-target", &format!("{arch}-apple-macos12.0"), "-module-name", "DeskMonkStore", "-o"])
        .arg(&lib)
        .arg("packaging/appstore/Store.swift")
        .status()
        .expect("xcrun swiftc runs (Xcode or the Command Line Tools)");
    assert!(status.success(), "Store.swift did not compile");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=dmstore");
    println!("cargo:rustc-link-lib=framework=StoreKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
    // The Swift runtime ships with macOS 12+; these are where the linker finds its stubs.
    let sdk = String::from_utf8(std::process::Command::new("xcrun").args(["--show-sdk-path"]).output().unwrap().stdout).unwrap();
    println!("cargo:rustc-link-search=native={}/usr/lib/swift", sdk.trim());
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    // Static back-deployment shims (swiftCompatibility*) sit next to the compiler.
    let swiftc = String::from_utf8(std::process::Command::new("xcrun").args(["-f", "swiftc"]).output().unwrap().stdout).unwrap();
    let toolchain = std::path::Path::new(swiftc.trim()).parent().unwrap().parent().unwrap();
    println!("cargo:rustc-link-search=native={}", toolchain.join("lib/swift/macosx").display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
