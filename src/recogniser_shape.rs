//! Guarded shape-edge normalization for the one-plan OCR recogniser.
use bitcoin_hashes::{Hash, sha256};
use serde_json::{Value, json};
use tract_onnx::{
    pb,
    prelude::*,
    tract_core::internal::{ensure, format_err},
};

/// Exact supported OCR model bytes; a different graph must be reviewed separately.
pub const MODEL_SHA256: &str = "e8770c967605983d1570cdf5352041dfb68fa0c21664f49f47b155abd3e0e318";
/// Dictionary paired with the supported model and CTC output classes.
pub const DICTIONARY_SHA256: &str =
    "5662df9d2d03f0e8ca0d3b0649d6acbab904b6a14b3d3521463c71c37c668ce3";
const SHAPE: &str = "penlock.r2.reshape79.shape";

/// Observations borrow preparation state; no graph or output is retained by
/// the scanner. The isolated probe uses these to preserve its measurement and
/// operator-listing boundaries while exercising the exact same builder.
#[cfg_attr(not(feature = "scan-profile"), allow(dead_code))]
pub enum BuildEvent<'a> {
    /// Named preparation stage is starting.
    StageStart(&'static str),
    /// Named preparation stage completed, in milliseconds.
    StageEnd(&'static str, f64),
    /// The single guarded shape edge has been rewritten.
    Rewritten,
    /// Borrow the typed graph for diagnostic evidence.
    Typed(&'a TypedModel),
    /// Borrow the exact symbolic dimension proof.
    Proven(&'a Value),
    /// Borrow the optimized graph for diagnostic operator listings.
    Optimized(&'a TypedModel),
}

/// Build one shared runnable through the guarded rewrite and typed shape proof.
pub fn build(
    bytes: &[u8],
    mut observe: impl FnMut(BuildEvent<'_>),
) -> TractResult<std::sync::Arc<TypedRunnableModel>> {
    let _build = crate::timing::span("ocr.plan_build");
    macro_rules! prepare {
        ($name:literal, $span:literal, $work:expr) => {{
            observe(BuildEvent::StageStart($name));
            let start = std::time::Instant::now();
            let span = crate::timing::span($span);
            let value = $work?;
            drop(span);
            observe(BuildEvent::StageEnd(
                $name,
                start.elapsed().as_secs_f64() * 1000.0,
            ));
            value
        }};
    }
    let mut proto = prepare!("parse_proto", "ocr.plan_parse", parse(bytes, [1, 3, 48]));
    prepare!("rewrite", "ocr.plan_rewrite", rewrite(&mut proto));
    observe(BuildEvent::Rewritten);
    let model = prepare!(
        "import",
        "ocr.plan_import",
        tract_onnx::onnx().model_for_proto_model(&proto)
    );
    drop(proto);
    let width = model.symbols.sym("ocr_r2_width");
    let model = prepare!(
        "symbolic_fact",
        "ocr.plan_fact",
        model.with_input_fact(
            0,
            f32::fact([1.to_dim(), 3.to_dim(), 48.to_dim(), width.to_dim()]).into(),
        )
    );
    let typed = prepare!("symbolic_type", "ocr.plan_type", model.into_typed());
    observe(BuildEvent::Typed(&typed));
    let proof = prepare!("shape_proof", "ocr.shape_proof", proof(&typed, &width));
    observe(BuildEvent::Proven(&proof));
    drop(proof);
    let optimized = prepare!(
        "symbolic_optimize",
        "ocr.plan_optimize",
        typed.into_optimized()
    );
    observe(BuildEvent::Optimized(&optimized));
    Ok(prepare!(
        "symbolic_runnable",
        "ocr.plan_runnable",
        optimized.into_runnable()
    ))
}

fn producer<'a>(
    g: &'a pb::GraphProto,
    output: &str,
    op: &str,
    inputs: &[&str],
) -> TractResult<&'a pb::NodeProto> {
    let nodes: Vec<_> = g
        .node
        .iter()
        .filter(|n| n.output.iter().any(|o| o == output))
        .collect();
    ensure!(nodes.len() == 1, "expected unique producer of {output}");
    let n = nodes[0];
    ensure!(
        n.domain.is_empty() && n.op_type == op && n.input == inputs && n.output == [output],
        "unexpected shape-branch contract at {output}"
    );
    Ok(n)
}

fn integer_attr(n: &pb::NodeProto, name: &str, value: i64) -> TractResult<()> {
    let attrs: Vec<_> = n.attribute.iter().filter(|a| a.name == name).collect();
    ensure!(
        attrs.len() == 1 && attrs[0].r#type == 2 && attrs[0].i == value,
        "unexpected {name} at {}",
        n.name
    );
    Ok(())
}

fn constant(g: &pb::GraphProto, output: &str, dtype: i32, value: i64) -> TractResult<()> {
    let n = producer(g, output, "Constant", &[])?;
    ensure!(
        n.attribute.len() == 1 && n.attribute[0].name == "value" && n.attribute[0].r#type == 4,
        "unexpected constant attributes at {output}"
    );
    let t = n.attribute[0]
        .t
        .as_ref()
        .ok_or_else(|| format_err!("missing constant {output}"))?;
    ensure!(
        t.dims == [1] && t.data_type == dtype && t.external_data.is_empty(),
        "unexpected constant type/shape at {output}"
    );
    let expected = if dtype == 6 {
        (value as i32).to_le_bytes().to_vec()
    } else {
        value.to_le_bytes().to_vec()
    };
    ensure!(
        t.raw_data == expected,
        "unexpected constant value at {output}"
    );
    Ok(())
}

/// Parse only the pinned asset with the expected input schema.
pub fn parse(bytes: &[u8], input_prefix: [usize; 3]) -> TractResult<pb::ModelProto> {
    ensure!(
        input_prefix == [1, 3, 48],
        "unsupported candidate input domain"
    );
    ensure!(
        sha256::Hash::hash(bytes).to_string() == MODEL_SHA256,
        "unsupported OCR asset hash"
    );
    tract_onnx::onnx().proto_model_for_read(&mut std::io::Cursor::new(bytes))
}

fn io_contract(g: &pb::GraphProto) -> TractResult<()> {
    ensure!(
        g.input.len() == 1
            && g.input[0].name == "x"
            && g.output.len() == 1
            && g.output[0].name == "softmax_2.tmp_0",
        "unexpected model inputs/outputs"
    );
    for (v, rank, fixed_axis, fixed) in [(&g.input[0], 4, 1, 3), (&g.output[0], 3, 2, 97)] {
        let tensor = match v.r#type.as_ref().and_then(|t| t.value.as_ref()) {
            Some(pb::type_proto::Value::TensorType(t)) => t,
            _ => return Err(format_err!("expected tensor at {}", v.name)),
        };
        let shape = tensor
            .shape
            .as_ref()
            .ok_or_else(|| format_err!("missing IO shape"))?;
        ensure!(
            tensor.elem_type == 1 && shape.dim.len() == rank,
            "unexpected IO dtype/rank"
        );
        ensure!(
            shape.dim[fixed_axis].value
                == Some(pb::tensor_shape_proto::dimension::Value::DimValue(fixed)),
            "unexpected IO fixed dimension"
        );
    }
    Ok(())
}

/// Also used directly by mutation tests; public entry point always hash-checks
/// before this structural guard. No numeric node index or heuristic fallback.
pub fn rewrite(proto: &mut pb::ModelProto) -> TractResult<()> {
    let g = proto
        .graph
        .as_ref()
        .ok_or_else(|| format_err!("missing graph"))?;
    io_contract(g)?;
    let targets: Vec<_> = g
        .node
        .iter()
        .enumerate()
        .filter(|(_, n)| n.name == "Reshape.79")
        .collect();
    ensure!(targets.len() == 1, "expected unique Reshape.79");
    let index = targets[0].0;
    ensure!(
        targets[0].1
            == producer(
                g,
                "reshape2_4.tmp_0",
                "Reshape",
                &["Add.219", "helper.concat.0"]
            )?,
        "unexpected reshape target"
    );
    integer_attr(targets[0].1, "allowzero", 0)?;
    let concat = producer(
        g,
        "helper.concat.0",
        "Concat",
        &["Cast.2", "Cast.4", "Cast.6", "Cast.8"],
    )?;
    ensure!(concat.name == "Concat.2", "unexpected concat name");
    integer_attr(concat, "axis", 0)?;
    for (output, input) in [
        ("Cast.2", "fill_constant_1.tmp_0"),
        ("Cast.4", "fill_constant_3.tmp_0"),
        ("Cast.6", "shape_0.tmp_0_slice_1"),
        ("Cast.8", "fill_constant_5.tmp_0"),
    ] {
        integer_attr(producer(g, output, "Cast", &[input])?, "to", 7)?;
    }
    for (name, value) in [
        ("fill_constant_1.tmp_0", 0),
        ("fill_constant_3.tmp_0", 1),
        ("fill_constant_5.tmp_0", 120),
    ] {
        constant(g, name, 6, value)?;
    }
    producer(
        g,
        "shape_0.tmp_0_slice_1",
        "Slice",
        &[
            "shape_0.tmp_0",
            "helper.constant.34",
            "helper.constant.35",
            "helper.constant.37",
            "helper.constant.36",
        ],
    )?;
    for (name, value) in [
        ("helper.constant.34", 3),
        ("helper.constant.35", 4),
        ("helper.constant.37", 0),
        ("helper.constant.36", 1),
    ] {
        constant(g, name, 7, value)?;
    }
    integer_attr(producer(g, "shape_0.tmp_0", "Cast", &["Shape.1"])?, "to", 6)?;
    producer(g, "Shape.1", "Shape", &["swish_8.tmp_0"])?;
    ensure!(
        !g.node
            .iter()
            .any(|n| n.name == SHAPE || n.input.iter().chain(&n.output).any(|s| s == SHAPE))
            && !g.initializer.iter().any(|t| t.name == SHAPE),
        "shape constant name collision"
    );
    let g = proto.graph.as_mut().unwrap();
    g.node[index].input[1] = SHAPE.into();
    // Add exactly one constant, before its user. Existing nodes/assets are intact.
    g.node.insert(
        index,
        pb::NodeProto {
            name: SHAPE.into(),
            op_type: "Constant".into(),
            output: vec![SHAPE.into()],
            attribute: vec![pb::AttributeProto {
                name: "value".into(),
                r#type: 4,
                t: Some(pb::TensorProto {
                    dims: vec![4],
                    data_type: 7,
                    raw_data: [0_i64, 1, -1, 120]
                        .into_iter()
                        .flat_map(i64::to_le_bytes)
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    Ok(())
}

fn check_shapes(
    feature: &[TDim],
    data: &[TDim],
    output: &[TDim],
    width: &Symbol,
) -> TractResult<Value> {
    ensure!(
        feature.len() == 4 && data.len() == 3 && output.len() == 4,
        "unexpected proof ranks"
    );
    ensure!(
        feature[0] == 1.into()
            && feature[1] == 120.into()
            && feature[2] == 1.into()
            && data[0] == 1.into()
            && data[2] == 120.into()
            && output[0] == 1.into()
            && output[1] == 1.into()
            && output[3] == 120.into(),
        "unexpected proof dimensions"
    );
    // Comparing only data with the rewritten output would be circular: compare
    // the original shape branch's feature-map T as well.
    ensure!(
        feature[3] == data[1] && data[1] == output[2],
        "T-preservation not symbolically established"
    );
    let mut times = Vec::new();
    for w in 16..=320 {
        let value = feature[3].eval_to_i64(&SymbolValues::default().with(width, w))?;
        ensure!(
            (1..=i32::MAX as i64).contains(&value),
            "unsupported T/cast range at W={w}"
        );
        times.push(value);
    }
    Ok(
        json!({"feature_shape": feature.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "reshape_data_shape": data.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "reshape_output_shape": output.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "symbolic_T_equal": true, "width_range": [16,320], "T_by_width": times,
        "normalized_shape": [0,1,-1,120]}),
    )
}

/// Prove the original intermediate and replacement output dimensions symbolically.
pub fn proof(model: &TypedModel, width: &Symbol) -> TractResult<Value> {
    let fact = |label| -> TractResult<Vec<TDim>> {
        let outlet = model
            .find_outlet_label(label)
            .ok_or_else(|| format_err!("missing proof label {label}"))?;
        Ok(model.outlet_fact(outlet)?.shape.to_tvec().to_vec())
    };
    check_shapes(
        &fact("swish_8.tmp_0")?,
        &fact("Add.219")?,
        &fact("reshape2_4.tmp_0")?,
        width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> Vec<u8> {
        include_bytes!("../models/text-recogniser.onnx").to_vec()
    }

    fn original() -> pb::ModelProto {
        parse(&asset(), [1, 3, 48]).unwrap()
    }

    #[test]
    fn only_one_edge_and_one_constant_change() {
        let before = original();
        let mut after = before.clone();
        rewrite(&mut after).unwrap();
        let g = after.graph.as_mut().unwrap();
        assert_eq!(g.node.len(), before.graph.as_ref().unwrap().node.len() + 1);
        let added = g.node.iter().position(|n| n.name == SHAPE).unwrap();
        assert_eq!(
            g.node.remove(added).attribute[0]
                .t
                .as_ref()
                .unwrap()
                .raw_data,
            [0_i64, 1, -1, 120]
                .into_iter()
                .flat_map(i64::to_le_bytes)
                .collect::<Vec<_>>()
        );
        g.node
            .iter_mut()
            .find(|n| n.name == "Reshape.79")
            .unwrap()
            .input[1] = "helper.concat.0".into();
        assert_eq!(after, before);
    }
    #[test]
    fn wrong_hash_and_input_domain_are_rejected() {
        assert!(parse(b"wrong asset", [1, 3, 48]).is_err());
        assert!(parse(&asset(), [1, 3, 32]).is_err());
        assert!(parse(&asset(), [2, 3, 48]).is_err());
    }
    #[test]
    fn malformed_graph_contracts_are_rejected_before_mutation() {
        for case in 0..7 {
            let mut p = original();
            let g = p.graph.as_mut().unwrap();
            let ix = g.node.iter().position(|n| n.name == "Reshape.79").unwrap();
            match case {
                0 => {
                    g.node.remove(ix);
                }
                1 => g.node.push(g.node[ix].clone()),
                2 => g.node[ix].input[0] = "wrong".into(),
                3 => g.node[ix].attribute[0].i = 1,
                4 => {
                    let n = g
                        .node
                        .iter_mut()
                        .find(|n| n.output == ["fill_constant_5.tmp_0"])
                        .unwrap();
                    n.attribute[0].t.as_mut().unwrap().raw_data = 119_i32.to_le_bytes().to_vec();
                }
                5 => {
                    let n = g
                        .node
                        .iter_mut()
                        .find(|n| n.output == ["shape_0.tmp_0_slice_1"])
                        .unwrap();
                    n.input.swap(1, 2);
                }
                _ => g.input[0].name = "wrong".into(),
            }
            let before = p.clone();
            assert!(rewrite(&mut p).is_err(), "case {case}");
            assert_eq!(p, before);
        }
    }
    #[test]
    fn symbolic_proof_checks_the_original_t_not_only_its_replacement() {
        let symbols = SymbolScope::default();
        let w = symbols.sym("W");
        let t = w.to_dim();
        let feature = vec![1.into(), 120.into(), 1.into(), t.clone()];
        let data = vec![1.into(), t.clone(), 120.into()];
        let out = vec![1.into(), 1.into(), t.clone(), 120.into()];
        assert_eq!(
            check_shapes(&feature, &data, &out, &w).unwrap()["T_by_width"]
                .as_array()
                .unwrap()
                .len(),
            305
        );
        let wrong_data = vec![1.into(), t.clone() + 1, 120.into()];
        let wrong_out = vec![1.into(), 1.into(), t + 1, 120.into()];
        assert!(check_shapes(&feature, &wrong_data, &wrong_out, &w).is_err());
    }
}
