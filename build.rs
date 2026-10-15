fn main() {
    println!("cargo::rustc-check-cfg=cfg(cuda_enabled)");
    println!("cargo:rerun-if-changed=src/gpu_kernel.cu");

    println!("cargo:rerun-if-env-changed=WORDLE_OPT_DISABLE_CUDA");

    if std::env::var("WORDLE_OPT_DISABLE_CUDA").is_err()
        && std::process::Command::new("nvcc")
            .arg("--version")
            .status()
            .is_ok()
    {
        cc::Build::new()
            .cuda(true)
            .flag("-ccbin=g++-13")
            .file("src/gpu_kernel.cu")
            .compile("wordlegpu");
        println!("cargo:rustc-cfg=cuda_enabled");
    } else {
        // Going quiet here is exactly how the compute host lost the GPU: with
        // no nvcc, `cuda_enabled` is never set, every #[cfg(cuda_enabled)] path
        // is compiled out, and the binary is CPU-only with nothing said about
        // it. Warn loudly and name the fix. Note this only prints when the
        // build script itself re-runs - `warn_if_compute_without_cuda` in
        // src/lib.rs is the runtime backstop for the case where nvcc is
        // installed later and cargo has no reason to re-run this script.
        println!(
            "cargo::warning=no nvcc found: building WITHOUT CUDA/GPU support \
             (all #[cfg(cuda_enabled)] paths are compiled out). On the compute \
             host this silently disables GPU acceleration - install \
             `nvidia-cuda-toolkit` and `g++-13` (this script passes \
             -ccbin=g++-13) and rebuild to re-enable it."
        );
    }
}
