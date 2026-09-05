//! Resolve task input/output names once, before capturing their typed keys.
//!
//! Named access remains available for convenience. The keyed success path
//! avoids name/hash lookup, but does not promise allocation-free execution.
//! Graph and binding operations deliberately panic in this API skeleton.

use futures_lite::future::block_on;
use slot_graph::{schema, Graph, Local, NodeOutputs, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let layout = schema! {
        ("value": u32, "factor": Optional<u32>) -> ("answer": u32)
    }
    .bind();
    let value = layout.input::<u32>("value")?;
    let factor = layout.input::<u32>("factor")?;
    let answer = layout.output::<u32>("answer")?;

    let mut graph = Graph::<Local>::new();
    let scale = graph.add_sync("scale", layout, move |_, inputs| {
        let value = inputs.required_key(value)?;
        let factor = inputs.optional_key(factor)?.map_or(2, |value| *value);
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(answer, *value * factor);
        Ok(outputs)
    })?;

    // Graph-facing handles are distinct from task keys. Names are convenient
    // here, outside task execution, for wiring and selecting report outputs.
    let external = graph.expose_input::<u32>(scale.input("value"))?;
    let result = graph.output::<u32>(scale, "answer")?;
    graph.set_active(scale, true)?;
    let version = graph.compile()?;
    let mut inputs = RunInputs::new();
    inputs.insert(external, 21_u32)?;
    let report = block_on(version.execute(inputs))?;
    assert_eq!(**report.output(result)?, 42);
    Ok(())
}
