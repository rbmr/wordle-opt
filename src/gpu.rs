
#[cfg(cuda_enabled)]
unsafe extern "C" {
    pub fn gpu_alloc_context() -> *mut std::ffi::c_void;
    pub fn gpu_free_context(ctx: *mut std::ffi::c_void);
    pub fn gpu_init(matrix: *const u8, matrix_size: usize, bounds: *const u32, bounds_size: usize);
    pub fn gpu_compute_phase1(
        ctx: *mut std::ffi::c_void,
        active_guesses: *const u16,
        num_active: i32,
        set: *const u16,
        set_len: i32,
        parent_max_k: i32,
        out_expected_rem: *mut u32,
        out_lb_cost: *mut u32,
        out_num_non_empty: *mut u8
    );
    pub fn gpu_get_h_active_guesses(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_set(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_out_expected_rem(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_lb_cost(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_num_non_empty(ctx: *mut std::ffi::c_void) -> *mut u8;
}

#[cfg(cuda_enabled)]
pub fn init_gpu_once(matrix: &[u8], bounds: &[Vec<u32>], max_k: usize) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
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
    });
}

#[cfg(cuda_enabled)]
pub struct GpuContextWrapper(pub *mut std::ffi::c_void);
#[cfg(cuda_enabled)]
unsafe impl Send for GpuContextWrapper {}
#[cfg(cuda_enabled)]
unsafe impl Sync for GpuContextWrapper {}

#[cfg(cuda_enabled)]
lazy_static::lazy_static! {
    pub static ref GPU_CTX_MUTEX: std::sync::Mutex<GpuContextWrapper> = std::sync::Mutex::new(GpuContextWrapper(unsafe { gpu_alloc_context() }));
}
