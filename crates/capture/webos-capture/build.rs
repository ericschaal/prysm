fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=webos-linker.sh");
    cc::Build::new()
        .cpp(true)
        .std("c++11")
        .file("native/vtcapture.cpp")
        .file("native/abi.cpp")
        .compile("vtcapture_exception_boundary");
}
