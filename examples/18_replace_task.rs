//! Replace a node factory without changing its Schema, edges, or old versions.
//! Graph operations intentionally panic in the current API skeleton.
use futures_lite::future::block_on;
use slot_graph::{outputs, schema, Graph, Local, RunInputs, Task};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let node = graph.add_sync("source", schema! { () -> ("value": u32) }, |_, _| {
        Ok(outputs! { "value" => 1_u32 })
    })?;
    graph.set_active(node, true)?;
    let output = graph.output::<u32>(node, "value")?;
    let v1 = graph.compile()?;
    graph.replace_task(
        node,
        Task::<Local>::asynchronous(|_, _| async {
            futures_lite::future::yield_now().await;
            Ok(outputs! { "value" => 2_u32 })
        }),
    )?;
    let v2 = graph.compile()?;
    let before = block_on(v1.execute(RunInputs::new()))?;
    let after = block_on(v2.execute(RunInputs::new()))?;
    assert_eq!(**before.output(output)?, 1);
    assert_eq!(**after.output(output)?, 2);
    Ok(())
}
