#[cfg(cuda_enabled)]
unsafe extern "C" {
    pub fn gpu_alloc_context() -> *mut std::ffi::c_void;
    pub fn gpu_free_context(ctx: *mut std::ffi::c_void);
    pub fn gpu_init(matrix: *const u8, matrix_size: usize, bounds: *const u32, bounds_size: usize);
    pub fn gpu_compute_phase1(
        ctx: *mut std::ffi::c_void,
        num_active: i32,
        set_len: i32,
        parent_max_k: i32,
    );
    pub fn gpu_get_h_active_guesses(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_set(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_out_expected_rem(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_lb_cost(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_num_non_empty(ctx: *mut std::ffi::c_void) -> *mut u8;
}

/// Uploads the response matrix (and capacity bounds) to the GPU, once per
/// distinct matrix. The engine can be used with more than one dictionary in a
/// single process (the test suite does exactly that), and the kernel indexes
/// the matrix by the full guess count, so re-initializing when the matrix
/// changes is required for correctness, not just an optimization.
#[cfg(cuda_enabled)]
pub fn init_gpu_once(matrix: &[u8], _bounds: &[Vec<u32>], _max_k: usize) {
    use std::sync::Mutex;

    fn content_key(matrix: &[u8]) -> (usize, u64) {
        use std::hash::{Hash, Hasher};
        let mut hasher = rustc_hash::FxHasher::default();
        matrix.hash(&mut hasher);
        (matrix.len(), hasher.finish())
    }

    static INIT: Mutex<Option<(usize, u64)>> = Mutex::new(None);
    let key = content_key(matrix);
    let mut guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if *guard == Some(key) {
        return;
    }
    let max_possible_k = 2340;
    let mut flat_bounds = vec![0u32; (max_possible_k + 1) * 2341];
    for k in 2..=max_possible_k {
        for c in 0..=2340 {
            flat_bounds[k * 2341 + c] = crate::heuristic::capacity_bound(c, k);
        }
    }
    unsafe {
        gpu_init(
            matrix.as_ptr(),
            matrix.len(),
            flat_bounds.as_ptr(),
            flat_bounds.len() * 4,
        );
    }
    *guard = Some(key);
}

#[cfg(cuda_enabled)]
pub struct GpuContextWrapper(pub *mut std::ffi::c_void);
#[cfg(cuda_enabled)]
unsafe impl Send for GpuContextWrapper {}
#[cfg(cuda_enabled)]
unsafe impl Sync for GpuContextWrapper {}

#[cfg(cuda_enabled)]
thread_local! {
    pub static GPU_CTX: std::cell::RefCell<GpuContextWrapper> = std::cell::RefCell::new(GpuContextWrapper(unsafe { gpu_alloc_context() }));
}
