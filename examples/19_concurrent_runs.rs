//! Drive two isolated input snapshots through one immutable version concurrently.
//! Graph operations intentionally panic in the current API skeleton.
use futures_lite::future::{block_on, yield_now, zip};
use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let node = graph.add_async(
        "double",
        schema! { ("value": u32) -> ("result": u32) },
        |_, inputs| async move {
            let value = inputs.required::<u32>("value")?;
            yield_now().await;
            Ok(outputs! { "result" => *value * 2 })
        },
    )?;
    let key = graph.expose_input::<u32>(node.input("value"))?;
    let output = graph.output::<u32>(node, "result")?;
    graph.set_active(node, true)?;
    let version = graph.compile()?;
    let mut left = RunInputs::new();
    left.insert(key, 10_u32)?;
    let mut right = RunInputs::new();
    right.insert(key, 20_u32)?;
    // zip polls both runs on this host thread; concurrent does not imply parallel.
    let (left, right) = block_on(zip(version.execute(left), version.execute(right)));
    assert_eq!(**left?.output(output)?, 20);
    assert_eq!(**right?.output(output)?, 40);
    Ok(())
}
