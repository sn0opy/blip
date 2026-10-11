fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Embed the icon into the Windows executable (Explorer, taskbar, shortcuts).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icons/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icons/icon.ico")
            .compile()
            .expect("failed to embed the Windows icon resource");
    }
}
