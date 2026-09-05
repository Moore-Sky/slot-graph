//! Scenario 09: edit a declaration graph and compile a new version.
//!
//! `v1` and `v2` are independent immutable execution versions. Editing affects
//! only versions compiled afterwards.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();

    let first = graph.add_sync(
        "first",
        schema! { () -> ("value": u32) },
        |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
    )?;
    let second = graph.add_sync(
        "second",
        schema! { ("value": u32) -> ("result": u32) },
        |_task, inputs| {
            let value = inputs.required::<u32>("value")?;
            Ok(outputs! { "result" => *value + 10 })
        },
    )?;
    let replacement = graph.add_sync(
        "replacement",
        schema! { () -> ("value": u32) },
        |_task, _inputs| Ok(outputs! { "value" => 100_u32 }),
    )?;

    let edge = graph.connect(first.output("value"), second.input("value"))?;
    graph.set_active(second, true)?;
    let result = graph.output::<u32>(second, "result")?;
    let v1 = graph.compile()?;

    // The already compiled v1 is not changed by this edit.
    graph.reconnect(edge, replacement.output("value"))?;
    let v2 = graph.compile()?;

    // Both versions run for real and produce different results: v1 still uses
    // `first`, while v2 uses `replacement`.
    let report_v1 = futures_lite::future::block_on(v1.execute(RunInputs::new()))?;
    let report_v2 = futures_lite::future::block_on(v2.execute(RunInputs::new()))?;
    println!("v1 result: {}", report_v1.output(result)?.as_ref());
    println!("v2 result: {}", report_v2.output(result)?.as_ref());

    Ok(())
}
