//! Explicit Slot identities, a Schema builder, and Many auto-collection.
//! Graph operations intentionally panic in the current API skeleton.
use futures_lite::future::block_on;
use slot_graph::{outputs, schema, Graph, InputSpec, Local, OutputSpec, RunInputs, Schema, SlotId};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let source = graph.add_sync("source", schema! { () -> ("a": u32, "b": u32) }, |_, _| {
        Ok(outputs! { "a" => 2_u32, "b" => 3_u32 })
    })?;
    let mut items = InputSpec::required_many::<u32>("items").auto_collect(true);
    items.id = SlotId::new(7);
    let mut sum = OutputSpec::new::<u32>("sum");
    sum.id = SlotId::new(8);
    let declaration = Schema::builder().input(items).output(sum).build();
    let collect = graph.add_sync("collect", declaration, |_, inputs| {
        let items = inputs.many::<u32>("items")?;
        Ok(outputs! { "sum" => items.iter().map(|v| **v).sum::<u32>() })
    })?;
    let connected = graph.connect_nodes(source, collect)?;
    assert_eq!(connected.edges.len(), 2);
    graph.set_active(collect, true)?;
    let sum = graph.output::<u32>(collect, "sum")?;
    let version = graph.compile()?;
    let report = block_on(version.execute(RunInputs::new()))?;
    assert_eq!(**report.output(sum)?, 5);
    Ok(())
}
