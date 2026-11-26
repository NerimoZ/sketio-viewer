fn main() {
    println!("cargo:rerun-if-changed=application.rc");
    println!("cargo:rerun-if-changed=ui/application.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("application.rc", ["SketioViewer"], embed_resource::NONE);
    }
}
