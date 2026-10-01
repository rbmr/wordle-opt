fn main() {
    println!("cargo::rustc-check-cfg=cfg(cuda_enabled)");
    println!("cargo:rerun-if-changed=src/gpu_kernel.cu");

    if std::process::Command::new("nvcc").arg("--version").status().is_ok() {
        cc::Build::new()
            .cuda(true)
            .flag("-ccbin=g++-13")
            .file("src/gpu_kernel.cu")
            .compile("wordlegpu");
        println!("cargo:rustc-cfg=cuda_enabled");
    }
}
