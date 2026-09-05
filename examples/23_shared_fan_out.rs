//! Scenario 23: forward the same non-Clone value through two consumers using insert_shared.

use futures_lite::future::block_on;
use slot_graph::{outputs, schema, Graph, Local, NodeOutputs, RunInputs};

struct Payload(Vec<u8>);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let source = graph.add_sync("source", schema! { () -> ("data": Payload) }, |_, _| {
        Ok(outputs! { "data" => Payload(vec![1, 2, 3]) })
    })?;
    let forward = graph.add_sync(
        "forward",
        schema! { ("data": Payload) -> ("data": Payload) },
        |_, inputs| {
            let data = inputs.required::<Payload>("data")?;
            let mut outputs = NodeOutputs::new();
            outputs.insert_shared("data", data);
            Ok(outputs)
        },
    )?;
    let count = graph.add_sync(
        "count",
        schema! { ("data": Payload) -> ("length": usize) },
        |_, inputs| {
            let data = inputs.required::<Payload>("data")?;
            Ok(outputs! { "length" => data.0.len() })
        },
    )?;
    graph.connect(source.output("data"), forward.input("data"))?;
    graph.connect(source.output("data"), count.input("data"))?;
    graph.set_active(forward, true)?;
    graph.set_active(count, true)?;
    let data_slot = graph.output::<Payload>(forward, "data")?;
    let count_slot = graph.output::<usize>(count, "length")?;
    let version = graph.compile()?;
    let mut report = block_on(version.execute(RunInputs::new()))?;
    let data = report.take_output(data_slot)?;
    let retained = data.clone(); // Payload itself does not implement Clone.
    assert_eq!(retained.0.len(), **report.output(count_slot)?);
    drop(report);
    assert_eq!(retained.0, vec![1, 2, 3]);
    Ok(())
}
