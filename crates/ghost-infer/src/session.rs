//! One ONNX Runtime environment per process and the session options the Python set:
//! CPU only, sequential execution, every graph optimization, and the thread counts each
//! wrapper chose (`intra_op_num_threads`, `inter_op_num_threads = 1`). The environment
//! loads the runtime's library from where `runtime` resolves it (1.24.4, the oracle's).

use std::{path::Path, sync::OnceLock};

use ort::{
    environment::Environment,
    session::{Session, SessionOutputs, builder::GraphOptimizationLevel},
};

use crate::{InferError, Result, runtime};

/// The process's ONNX Runtime environment, created on first use. `ort` allows one per
/// process, so every model shares it. The library is resolved once here, before the first
/// session (`runtime::resolve_for_process`), and its path logged.
pub fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let library = runtime::resolve_for_process().map_err(|e| e.to_string())?;
        tracing::info!("onnxruntime: {}", library.display());
        ort::init_from(&library)
            .map_err(|e| e.to_string())?
            .with_name("ghost")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| InferError::Model(format!("onnxruntime environment: {e}")))
}

/// `onnxruntime.SessionOptions` as the Python wrappers set them.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// `intra_op_num_threads`; 0 leaves ONNX Runtime's default (the physical cores), which
    /// is what `kokoro-onnx` runs with.
    pub intra_threads: usize,
    /// `inter_op_num_threads`; every wrapper sets 1.
    pub inter_threads: usize,
    /// `add_session_config_entry` pairs (livekit-wakeword's `mlas.disable_kleidiai`).
    pub entries: Vec<(&'static str, &'static str)>,
}

impl SessionConfig {
    /// `intra_op_num_threads = threads`, `inter_op_num_threads = 1`.
    pub fn threads(threads: usize) -> Self {
        Self {
            intra_threads: threads,
            inter_threads: 1,
            entries: Vec::new(),
        }
    }
}

/// Open a model file on the CPU with `config`.
pub fn open(path: &Path, config: &SessionConfig) -> Result<Session> {
    let mut builder = Session::builder(environment()?)?
        .with_inter_threads(config.inter_threads)
        .map_err(ort::Error::from)?
        .with_parallel_execution(false)
        .map_err(ort::Error::from)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort::Error::from)?;
    if config.intra_threads > 0 {
        builder = builder
            .with_intra_threads(config.intra_threads)
            .map_err(ort::Error::from)?;
    }
    for (key, value) in &config.entries {
        builder = builder
            .with_config_entry(key, value)
            .map_err(ort::Error::from)?;
    }
    Ok(builder.commit_from_file(path)?)
}

/// A named `float32` output, copied out with its shape.
pub(crate) fn output_f32(outputs: &SessionOutputs<'_>, name: &str) -> Result<(Vec<i64>, Vec<f32>)> {
    let value = outputs
        .get(name)
        .ok_or_else(|| InferError::Model(format!("the model has no output {name:?}")))?;
    let (shape, data) = value.try_extract_tensor::<f32>()?;
    Ok((shape.to_vec(), data.to_vec()))
}

/// A named `int64` output, copied out.
pub(crate) fn output_i64(outputs: &SessionOutputs<'_>, name: &str) -> Result<Vec<i64>> {
    let value = outputs
        .get(name)
        .ok_or_else(|| InferError::Model(format!("the model has no output {name:?}")))?;
    let (_, data) = value.try_extract_tensor::<i64>()?;
    Ok(data.to_vec())
}

/// The model's first output as `float32`, with its shape (the wrappers that run
/// `session.run(None, ...)` read `outputs[0]`).
pub(crate) fn first_output_f32(outputs: &SessionOutputs<'_>) -> Result<(Vec<i64>, Vec<f32>)> {
    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    Ok((shape.to_vec(), data.to_vec()))
}

/// The name of the model's first input (the wake classifiers are run by it).
pub(crate) fn first_input_name(session: &Session) -> Result<String> {
    session
        .inputs()
        .first()
        .map(|input| input.name().to_string())
        .ok_or_else(|| InferError::Model("the model has no inputs".into()))
}
