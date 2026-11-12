//! CUDA backend of the clustering kernel's assignment step (MEDULLA-CLUSTER-R008.3,
//! CONCEPT:MEDULLA-KG.compute.gpu-distance-seam / MEDULLA-327).
//!
//! The kernel in `medulla_numeric::cluster` owns training. Its assignment step (for
//! each row, scan `k` centroids and keep the nearest) is the one hot loop worth a
//! device. This module registers a CUDA implementation of the kernel's
//! [`AssignBackend`] when a device initialises. Any device, compile or launch failure
//! degrades that call to the CPU path, so a `gpu-cuda` binary on a GPU-less host stays
//! correct. A backend must return the same label as the CPU path for every row.
//!
//! The module exists only under `gpu-cuda`; `cudarc` loads the driver at run time, so
//! no CUDA toolkit is needed to build.

use cudarc::driver::{CudaContext, CudaFunction, LaunchConfig, PushKernelArg};
use medulla_numeric::cluster::{set_assign_backend, AssignBackend, CpuBackend, Rows};
use std::sync::{Arc, OnceLock};

/// One thread scores one data point against every centroid (full squared-L2, no early
/// exit) and writes the argmin index and its distance.
const KERNEL_SRC: &str = r#"
extern "C" __global__ void batch_assign(
        const float* data, const float* centroids, int* out, float* out_dist,
        const int n, const int k, const int dim) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    const float* v = data + (size_t)i * dim;
    float best = 3.402823466e38f;
    int best_idx = 0;
    for (int c = 0; c < k; c++) {
        const float* cent = centroids + (size_t)c * dim;
        float s = 0.0f;
        for (int d = 0; d < dim; d++) {
            float diff = v[d] - cent[d];
            s += diff * diff;
        }
        if (s < best) {
            best = s;
            best_idx = c;
        }
    }
    out[i] = best_idx;
    out_dist[i] = best;
}
"#;

/// The initialised CUDA backend (context + compiled kernel).
pub struct CudaAssign {
    ctx: Arc<CudaContext>,
    func: CudaFunction,
}

impl CudaAssign {
    fn init(ordinal: usize) -> Result<Self, String> {
        let ctx = CudaContext::new(ordinal).map_err(|e| format!("cuda ctx: {e:?}"))?;
        let ptx = cudarc::nvrtc::compile_ptx(KERNEL_SRC).map_err(|e| format!("nvrtc: {e:?}"))?;
        let module = ctx
            .load_module(ptx)
            .map_err(|e| format!("load_module: {e:?}"))?;
        let func = module
            .load_function("batch_assign")
            .map_err(|e| format!("load_function: {e:?}"))?;
        Ok(Self { ctx, func })
    }

    /// Gather the named rows, run the kernel, return labels and distances.
    fn try_assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        centroids: &[f32],
        k: usize,
    ) -> Result<(Vec<u32>, Vec<f32>), String> {
        let (n, dim) = (idx.len(), rows.dim);
        let mut gathered = Vec::with_capacity(n * dim);
        for &i in idx {
            gathered.extend_from_slice(rows.row(i as usize));
        }
        let stream = self.ctx.default_stream();
        let d_data = stream
            .memcpy_stod(&gathered)
            .map_err(|e| format!("htod data: {e:?}"))?;
        let d_cent = stream
            .memcpy_stod(centroids)
            .map_err(|e| format!("htod centroids: {e:?}"))?;
        let mut d_out = stream
            .alloc_zeros::<i32>(n)
            .map_err(|e| format!("alloc out: {e:?}"))?;
        let mut d_dist = stream
            .alloc_zeros::<f32>(n)
            .map_err(|e| format!("alloc dist: {e:?}"))?;
        let (n_i32, k_i32, dim_i32) = (n as i32, k as i32, dim as i32);
        let cfg = LaunchConfig::for_num_elems(n as u32);
        let mut builder = stream.launch_builder(&self.func);
        builder
            .arg(&d_data)
            .arg(&d_cent)
            .arg(&mut d_out)
            .arg(&mut d_dist)
            .arg(&n_i32)
            .arg(&k_i32)
            .arg(&dim_i32);
        // SAFETY: the argument list matches the kernel signature above and every
        // device buffer outlives the launch and the copies back.
        unsafe { builder.launch(cfg) }.map_err(|e| format!("launch: {e:?}"))?;
        let labels = stream
            .memcpy_dtov(&d_out)
            .map_err(|e| format!("dtoh out: {e:?}"))?;
        let dists = stream
            .memcpy_dtov(&d_dist)
            .map_err(|e| format!("dtoh dist: {e:?}"))?;
        Ok((labels.into_iter().map(|x| x as u32).collect(), dists))
    }
}

impl AssignBackend for CudaAssign {
    fn name(&self) -> &'static str {
        "cuda"
    }

    fn assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        centroids: &[f32],
        k: usize,
        labels: &mut [u32],
        dists: &mut [f32],
    ) {
        if idx.is_empty() || k == 0 || rows.dim == 0 {
            return CpuBackend.assign(rows, idx, centroids, k, labels, dists);
        }
        match self.try_assign(rows, idx, centroids, k) {
            Ok((l, d)) => {
                labels.copy_from_slice(&l);
                dists.copy_from_slice(&d);
            }
            Err(e) => {
                tracing::warn!("cuda batch_assign failed ({e}); CPU fallback");
                CpuBackend.assign(rows, idx, centroids, k, labels, dists);
            }
        }
    }
}

/// Register the CUDA backend with the kernel when a device initialises. Returns
/// whether a device is in use. `MEDULLA_ANN_DEVICE` selects the ordinal (default 0).
/// Runs the probe once per process.
pub fn register_cuda_backend() -> bool {
    static ACTIVE: OnceLock<bool> = OnceLock::new();
    *ACTIVE.get_or_init(|| {
        let ordinal = std::env::var("MEDULLA_ANN_DEVICE")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        // cudarc's `dynamic-loading` panics (does not return `Err`) when libcuda cannot
        // be loaded, i.e. on any GPU-less host. Catch that unwind under a silenced hook.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let res = std::panic::catch_unwind(|| CudaAssign::init(ordinal));
        std::panic::set_hook(prev);
        match res {
            Ok(Ok(backend)) => {
                set_assign_backend(Arc::new(backend));
                true
            }
            Ok(Err(e)) => {
                tracing::info!("CUDA assignment backend unavailable ({e}); using CPU");
                false
            }
            Err(_) => {
                tracing::info!("CUDA driver not loadable; using the CPU assignment path");
                false
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a host with no usable device (every build host and CI runner) the probe
    /// absorbs the driver-load failure and the CPU path stays active.
    #[cfg(not(medulla_gpu_device_tests))]
    #[test]
    fn cuda_probe_without_a_device_falls_back_to_cpu() {
        assert!(
            !register_cuda_backend(),
            "a CUDA device initialised; run the device parity test instead: \
             RUSTFLAGS=\"--cfg medulla_gpu_device_tests\" cargo test -p medulla-ann --features gpu-cuda"
        );
        assert_eq!(medulla_numeric::cluster::active_backend().name(), "cpu");
    }

    /// Device parity: the CUDA labels equal the CPU labels for every row. Compiled only
    /// under the opt-in `--cfg medulla_gpu_device_tests`, and fails when no device
    /// initialises.
    #[cfg(medulla_gpu_device_tests)]
    #[test]
    fn cuda_assign_matches_cpu_ground_truth() {
        assert!(
            register_cuda_backend(),
            "no CUDA device initialised on this host"
        );
        let (dim, k, n) = (32usize, 64usize, 4096usize);
        let cents: Vec<f32> = (0..k * dim)
            .map(|i| ((i * 37) % 200) as f32 - 100.0)
            .collect();
        let data: Vec<f32> = (0..n * dim)
            .map(|i| cents[(i / dim % k) * dim + i % dim] + (i % 7) as f32 * 0.01)
            .collect();
        let rows = Rows::new(&data, n, dim);
        let idx: Vec<u32> = (0..n as u32).collect();
        let (mut gl, mut gd) = (vec![0u32; n], vec![0f32; n]);
        medulla_numeric::cluster::active_backend().assign(rows, &idx, &cents, k, &mut gl, &mut gd);
        let (mut cl, mut cd) = (vec![0u32; n], vec![0f32; n]);
        CpuBackend.assign(rows, &idx, &cents, k, &mut cl, &mut cd);
        assert_eq!(gl, cl);
    }
}
