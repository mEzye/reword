fn main() {
    slint_build::compile("ui/reword.slint").expect("Could not compile the Reword interface");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        let object =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("reword-icon.o");
        let status = std::process::Command::new("windres")
            .args(["-i", "assets/reword.rc", "-O", "coff", "-o"])
            .arg(&object)
            .status()
            .expect("windres is required to embed the app icon");
        assert!(status.success(), "Could not embed the app icon");
        println!("cargo:rustc-link-arg={}", object.display());
        println!("cargo:rerun-if-changed=assets/reword.rc");
        println!("cargo:rerun-if-changed=assets/Reword.ico");
    }
}
