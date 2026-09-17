//! Shared single-output ONNX execution contract.

/// Executes one input and requires exactly one float output tensor.
#[cfg(feature = "onnx")]
pub fn run_plan(
    plan: &std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>,
    input: tract_onnx::prelude::Tensor,
) -> Result<Vec<f32>, String> {
    use tract_onnx::prelude::*;
    let result = plan.run(tvec!(input.into())).map_err(|e| e.to_string())?;
    let [only] = result.as_slice() else {
        return Err(format!(
            "the model produced {} output tensors; the contract is one",
            result.len()
        ));
    };
    let view = only
        .to_plain_array_view::<f32>()
        .map_err(|e| e.to_string())?;
    Ok(view.iter().copied().collect())
}
