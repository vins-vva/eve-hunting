//! Embeds the app's icon (`assets/icon.ico`, from `assets/icon.svg`) in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/icon.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compiling assets/icon.rc (needs the Windows SDK's rc.exe)");
    }
}
