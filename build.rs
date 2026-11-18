//! Gives the Windows program its icon and the details shown under
//! Properties > Details. Other systems need nothing here: the Mac icon and
//! `Info.plist` are made by `scripts/macos-app.sh`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/tapprint.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/tapprint.ico")
        .set("ProductName", "TapPrint")
        .set("FileDescription", "TapPrint receipt printing helper")
        .set("CompanyName", "TapPrint")
        .set("LegalCopyright", "MIT licence")
        .set("OriginalFilename", "tapprint.exe");
    if let Err(error) = resource.compile() {
        // A program with no icon would still work, but a release must not
        // quietly ship without its details, so the build stops here.
        eprintln!("could not add the Windows icon and details: {error}");
        std::process::exit(1);
    }
}
