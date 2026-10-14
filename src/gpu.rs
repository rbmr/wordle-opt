#[cfg(cuda_enabled)]
unsafe extern "C" {
    pub fn gpu_alloc_context() -> *mut std::ffi::c_void;
    pub fn gpu_free_context(ctx: *mut std::ffi::c_void);
    pub fn gpu_init_bounds(bounds: *const u32, bounds_size: usize);
    pub fn gpu_upload_matrix(matrix: *const u8, matrix_size: usize) -> *const u8;
    pub fn gpu_free_matrix(matrix: *const u8);
    pub fn gpu_compute_phase1(
        ctx: *mut std::ffi::c_void,
        num_active: i32,
        set_len: i32,
        parent_max_k: i32,
        d_matrix: *const u8,
        num_guesses: i32,
    );
    pub fn gpu_get_h_active_guesses(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_set(ctx: *mut std::ffi::c_void) -> *mut u16;
    pub fn gpu_get_h_out_expected_rem(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_lb_cost(ctx: *mut std::ffi::c_void) -> *mut u32;
    pub fn gpu_get_h_out_num_non_empty(ctx: *mut std::ffi::c_void) -> *mut u8;
}

#[cfg(cuda_enabled)]
static INIT_BOUNDS: std::sync::Once = std::sync::Once::new();

#[cfg(cuda_enabled)]
pub fn ensure_bounds_init() {
    INIT_BOUNDS.call_once(|| {
        let max_possible_k = 2340;
        let mut flat_bounds = vec![0u32; (max_possible_k + 1) * 2341];
        for k in 2..=max_possible_k {
            for c in 0..=2340 {
                flat_bounds[k * 2341 + c] = crate::heuristic::capacity_bound(c, k);
            }
        }
        unsafe {
            gpu_init_bounds(
                flat_bounds.as_ptr(),
                flat_bounds.len() * std::mem::size_of::<u32>(),
            );
        }
    });
}

/// A device allocation of the response matrix on the GPU.
/// Owned by `ResponseMatrix`, guaranteeing that concurrent solves with different
/// dictionaries access their own distinct device allocations without races.
#[cfg(cuda_enabled)]
pub struct DeviceMatrix {
    ptr: *const u8,
}

#[cfg(cuda_enabled)]
unsafe impl Send for DeviceMatrix {}
#[cfg(cuda_enabled)]
unsafe impl Sync for DeviceMatrix {}

#[cfg(cuda_enabled)]
impl DeviceMatrix {
    pub fn new(matrix: &[u8]) -> Option<Self> {
        ensure_bounds_init();
        let ptr = unsafe { gpu_upload_matrix(matrix.as_ptr(), matrix.len()) };
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }
}

#[cfg(cuda_enabled)]
impl Drop for DeviceMatrix {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                gpu_free_matrix(self.ptr);
            }
        }
    }
}

#[cfg(cuda_enabled)]
pub struct GpuContextWrapper(pub *mut std::ffi::c_void);
#[cfg(cuda_enabled)]
unsafe impl Send for GpuContextWrapper {}
#[cfg(cuda_enabled)]
unsafe impl Sync for GpuContextWrapper {}

#[cfg(cuda_enabled)]
impl Drop for GpuContextWrapper {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { gpu_free_context(self.0) };
        }
    }
}

#[cfg(cuda_enabled)]
thread_local! {
    pub static GPU_CTX: std::cell::RefCell<GpuContextWrapper> = std::cell::RefCell::new(GpuContextWrapper(unsafe { gpu_alloc_context() }));
}
