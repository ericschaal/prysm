fn main() {
    println!("cargo:rerun-if-changed=native");
    // abi.cpp only backs the layout and exception-boundary tests; it is tiny and
    // compiles for both the TV and the development host.
    cc::Build::new()
        .cpp(true)
        .std("c++11")
        .file("native/vtcapture.cpp")
        .file("native/abi.cpp")
        .compile("vtcapture_exception_boundary");
}
