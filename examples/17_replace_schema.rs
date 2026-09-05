//! Scenario 17: rename a Slot while preserving its identity, edge, and old compiled version.
use futures_lite::future::block_on;
use slot_graph::{
    outputs, schema, EditErrorKind, Graph, InputSpec, Local, OutputSpec, RunInputs, Schema, SlotId,
    Task,
};

fn consumer_schema(name: &str) -> Schema {
    let mut input = InputSpec::required_one::<u32>(name);
    input.id = SlotId::new(7);
    Schema::new(vec![input], vec![OutputSpec::new::<u32>("result")])
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let source = graph.add_sync("source", schema! { () -> ("value": u32) }, |_, _| {
        Ok(outputs! { "value" => 10_u32 })
    })?;
    let consumer = graph.add_sync("consumer", consumer_schema("before"), |_, inputs| {
        Ok(outputs! { "result" => *inputs.required::<u32>("before")? })
    })?;
    let edge = graph.connect(source.output("value"), consumer.input("before"))?;
    let old_input = graph.input::<u32>(consumer, "before")?;
    let old_output = graph.output::<u32>(consumer, "result")?;
    graph.set_active(consumer, true)?;
    let v1 = graph.compile()?;

    let changes = graph.replace_schema(
        consumer,
        consumer_schema("after"),
        Task::<Local>::sync(|_, inputs| {
            Ok(outputs! { "result" => *inputs.required::<u32>("after")? + 1 })
        }),
    )?;
    assert!(changes.removed_edges.is_empty());
    let error = graph
        .connect(source.output("value"), old_input)
        .unwrap_err();
    assert_eq!(error.kind, EditErrorKind::StaleSlotHandle);
    let new_output = graph.output::<u32>(consumer, "result")?;
    let v2 = graph.compile()?;
    assert_eq!(
        **block_on(v1.execute(RunInputs::new()))?.output(old_output)?,
        10
    );
    assert_eq!(
        **block_on(v2.execute(RunInputs::new()))?.output(new_output)?,
        11
    );
    graph.disconnect(edge)?; // The preserved connection retains its original EdgeId.
    Ok(())
}
