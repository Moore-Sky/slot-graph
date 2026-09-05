//! Scenario 01: the smallest synchronous data flow.
//!
//! `consume` runs only after `produce` has committed its value.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() {
    basic_sync();
}

fn basic_sync() {
    let mut graph = Graph::<Local>::new();

    let produce = graph
        .add_sync(
            "produce",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok(outputs! { "value" => 42_u32 }),
        )
        .unwrap();

    let consume = graph
        .add_sync(
            "consume",
            schema! { ("value": u32) -> () },
            |_task, inputs| {
                let value = inputs.required::<u32>("value")?;
                assert_eq!(*value, 42);
                Ok(outputs! {})
            },
        )
        .unwrap();

    graph
        .connect(produce.output("value"), consume.input("value"))
        .unwrap();
    graph.set_active(consume, true).unwrap();

    let version = graph.compile().unwrap();
    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
}
