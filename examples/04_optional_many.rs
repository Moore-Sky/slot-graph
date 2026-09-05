//! Scenario 04: Optional and Many are independent dimensions.
//!
//! `metadata` may have no source. `items` is Required + Many, so it waits for
//! every connected producer. The current skeleton deliberately panics when run.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() {
    optional_and_many();
}

fn optional_and_many() {
    let mut graph = Graph::<Local>::new();

    let first = graph
        .add_sync(
            "first",
            schema! { () -> ("item": u32) },
            |_task, _inputs| Ok(outputs! { "item" => 10_u32 }),
        )
        .unwrap();
    let second = graph
        .add_sync(
            "second",
            schema! { () -> ("item": u32) },
            |_task, _inputs| Ok(outputs! { "item" => 20_u32 }),
        )
        .unwrap();

    let merge = graph
        .add_sync(
            "merge",
            schema! {
                (
                    "metadata": Optional<String>,
                    "items": Many<u32>,
                ) -> ("sum": u32)
            },
            |_task, inputs| {
                let metadata = inputs.optional::<String>("metadata")?;
                assert!(metadata.is_none());
                let items = inputs.many::<u32>("items")?;
                Ok(outputs! {
                    "sum" => items.iter().map(|item| **item).sum::<u32>()
                })
            },
        )
        .unwrap();

    graph
        .connect(first.output("item"), merge.input("items"))
        .unwrap();
    graph
        .connect(second.output("item"), merge.input("items"))
        .unwrap();
    graph.set_active(merge, true).unwrap();

    let version = graph.compile().unwrap();
    let _ = futures_lite::future::block_on(version.execute(RunInputs::new()));
}
