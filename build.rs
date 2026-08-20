//! Embeds the Windows icon resource. Everything else about the build is
//! ordinary cargo, and this file should stay that small.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/jutsu-audio.rc");
    println!("cargo:rerun-if-changed=assets/icon/jutsu-audio.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/icon/jutsu-audio.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embedding assets/icon/jutsu-audio.rc");
    }
}
