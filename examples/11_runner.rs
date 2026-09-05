//! Scenario 11: a frame loop reuses CPU-side run storage through an exclusive
//! runner.
//!
//! The runner does not own GPU fences; the host still owns output-resource
//! lifetimes.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let frame = graph.add_sync(
        "frame",
        schema! { () -> ("frame_index": u64) },
        |_task, _inputs| Ok(outputs! { "frame_index" => 1_u64 }),
    )?;
    graph.set_active(frame, true)?;

    let version = graph.compile()?;
    let mut runner = version.runner();

    // `execute` is shorthand for `start + await`.
    let report = futures_lite::future::block_on(runner.execute(RunInputs::new()))?;
    println!("frame status: {:?}", report.status(frame));

    Ok(())
}
