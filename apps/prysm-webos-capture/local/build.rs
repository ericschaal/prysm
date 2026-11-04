fn main() {
    println!("cargo:rerun-if-changed=../native");
    let mut native = cc::Build::new();
    if std::env::var_os("CARGO_FEATURE_WEBOS_NEON").is_some()
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("arm")
    {
        println!("cargo:rerun-if-changed=nv12.cpp");
        native.file("nv12.cpp").flag("-mfpu=neon");
    }
    native
        .cpp(true)
        .std("c++11")
        .file("../native/vtcapture.cpp")
        .file("../native/abi.cpp")
        .compile("vtcapture_exception_boundary");
}
