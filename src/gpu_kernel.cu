
#include <cuda_runtime.h>
#include <stdint.h>
#include <stdio.h>

unsigned char* global_d_matrix = NULL;
uint32_t* global_d_bounds = NULL;

extern "C" {

struct ThreadContext {
    cudaStream_t stream;
    uint16_t* d_active_guesses;
    uint16_t* d_set;
    uint32_t* d_out_expected_rem;
    uint32_t* d_out_lb_cost;
    uint8_t* d_out_num_non_empty;

    uint16_t* h_active_guesses;
    uint16_t* h_set;
    uint32_t* h_out_expected_rem;
    uint32_t* h_out_lb_cost;
    uint8_t*  h_out_num_non_empty;
};

void* gpu_alloc_context() {
    ThreadContext* ctx = (ThreadContext*)malloc(sizeof(ThreadContext));
    cudaSetDevice(0);
    cudaStreamCreate(&ctx->stream);
    
    cudaHostAlloc(&ctx->h_active_guesses, 14855 * sizeof(uint16_t), cudaHostAllocDefault);
    cudaHostAlloc(&ctx->h_set, 2340 * sizeof(uint16_t), cudaHostAllocDefault);
    cudaHostAlloc(&ctx->h_out_expected_rem, 14855 * sizeof(uint32_t), cudaHostAllocDefault);
    cudaHostAlloc(&ctx->h_out_lb_cost, 14855 * sizeof(uint32_t), cudaHostAllocDefault);
    cudaHostAlloc(&ctx->h_out_num_non_empty, 14855 * sizeof(uint8_t), cudaHostAllocDefault);

    cudaMalloc(&ctx->d_active_guesses, 14855 * sizeof(uint16_t));
    cudaMalloc(&ctx->d_set, 2340 * sizeof(uint16_t));
    cudaMalloc(&ctx->d_out_expected_rem, 14855 * sizeof(uint32_t));
    cudaMalloc(&ctx->d_out_lb_cost, 14855 * sizeof(uint32_t));
    cudaMalloc(&ctx->d_out_num_non_empty, 14855 * sizeof(uint8_t));
    
    return ctx;
}

void gpu_free_context(void* ptr) {
    ThreadContext* ctx = (ThreadContext*)ptr;
    cudaFree(ctx->d_active_guesses);
    cudaFree(ctx->d_set);
    cudaFree(ctx->d_out_expected_rem);
    cudaFree(ctx->d_out_lb_cost);
    cudaFree(ctx->d_out_num_non_empty);
    
    cudaFreeHost(ctx->h_active_guesses);
    cudaFreeHost(ctx->h_set);
    cudaFreeHost(ctx->h_out_expected_rem);
    cudaFreeHost(ctx->h_out_lb_cost);
    cudaFreeHost(ctx->h_out_num_non_empty);

    cudaStreamDestroy(ctx->stream);
    free(ctx);
}

void gpu_init(unsigned char* host_matrix, size_t matrix_size, uint32_t* host_bounds, size_t bounds_size) {
    cudaSetDeviceFlags(cudaDeviceScheduleBlockingSync);
    cudaSetDevice(0);
    cudaMalloc(&global_d_matrix, matrix_size);
    cudaMemcpy(global_d_matrix, host_matrix, matrix_size, cudaMemcpyHostToDevice);

    cudaMalloc(&global_d_bounds, bounds_size);
    cudaMemcpy(global_d_bounds, host_bounds, bounds_size, cudaMemcpyHostToDevice);
}

} // extern "C"

__global__ void gpu_compute_phase1_kernel(
    const uint16_t* active_guesses,
    int num_active,
    const uint16_t* set,
    int set_len,
    int parent_max_k,
    const unsigned char* g_matrix,
    const uint32_t* g_capacity_bounds,
    uint32_t* out_expected_rem,
    uint32_t* out_lb_cost,
    uint8_t* out_num_non_empty
) {
    int g_idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (g_idx >= num_active) return;
    int g = active_guesses[g_idx];

    uint16_t counts[243];
    for (int i = 0; i < 243; i++) counts[i] = 0;

    for (int i = 0; i < set_len; i++) {
        int c = set[i];
        int r = g_matrix[c * 14855 + g];
        counts[r]++;
    }

    uint32_t expected_rem = 0;
    uint32_t lb_cost = set_len;
    uint8_t num_non_empty = 0;

    for (int r = 0; r < 243; r++) {
        uint16_t count = counts[r];
        if (count > 0) {
            num_non_empty++;
            expected_rem += (uint32_t)count * (uint32_t)count;
            if (r != 121) {
                lb_cost += g_capacity_bounds[parent_max_k * 2341 + count];
            }
        }
    }

    out_expected_rem[g_idx] = expected_rem;
    out_lb_cost[g_idx] = lb_cost;
    out_num_non_empty[g_idx] = num_non_empty;
}

extern "C" {
void gpu_compute_phase1(
    ThreadContext* ctx,
    const uint16_t* host_active_guesses,
    int num_active,
    const uint16_t* host_set,
    int set_len,
    int parent_max_k,
    uint32_t* host_out_expected_rem,
    uint32_t* host_out_lb_cost,
    uint8_t* host_out_num_non_empty
) {
    cudaMemcpyAsync(ctx->d_active_guesses, ctx->h_active_guesses, num_active * sizeof(uint16_t), cudaMemcpyHostToDevice, ctx->stream);
    cudaMemcpyAsync(ctx->d_set, ctx->h_set, set_len * sizeof(uint16_t), cudaMemcpyHostToDevice, ctx->stream);

    int block = 64;
    int grid = (num_active + block - 1) / block;

    gpu_compute_phase1_kernel<<<grid, block, 0, ctx->stream>>>(
        ctx->d_active_guesses,
        num_active,
        ctx->d_set,
        set_len,
        parent_max_k,
        global_d_matrix,
        global_d_bounds,
        ctx->d_out_expected_rem,
        ctx->d_out_lb_cost,
        ctx->d_out_num_non_empty
    );

    cudaMemcpyAsync(ctx->h_out_expected_rem, ctx->d_out_expected_rem, num_active * sizeof(uint32_t), cudaMemcpyDeviceToHost, ctx->stream);
    cudaMemcpyAsync(ctx->h_out_lb_cost, ctx->d_out_lb_cost, num_active * sizeof(uint32_t), cudaMemcpyDeviceToHost, ctx->stream);
    cudaMemcpyAsync(ctx->h_out_num_non_empty, ctx->d_out_num_non_empty, num_active * sizeof(uint8_t), cudaMemcpyDeviceToHost, ctx->stream);

    cudaStreamSynchronize(ctx->stream);
}

    uint16_t* gpu_get_h_active_guesses(ThreadContext* ctx) { return ctx->h_active_guesses; }
    uint16_t* gpu_get_h_set(ThreadContext* ctx) { return ctx->h_set; }
    uint32_t* gpu_get_h_out_expected_rem(ThreadContext* ctx) { return ctx->h_out_expected_rem; }
    uint32_t* gpu_get_h_out_lb_cost(ThreadContext* ctx) { return ctx->h_out_lb_cost; }
    uint8_t*  gpu_get_h_out_num_non_empty(ThreadContext* ctx) { return ctx->h_out_num_non_empty; }
}
