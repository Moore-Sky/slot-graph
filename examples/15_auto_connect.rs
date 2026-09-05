//! Scenario 15: when schemas are clear, `connect_nodes` creates edges for
//! exact name/type matches.
//!
//! Explicit `connect` remains authoritative. Ambiguous auto-connection returns
//! an error without leaving a partial set of edges.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let gbuffer = graph.add_sync(
        "gbuffer",
        schema! {
            () -> (
                "color": u32,
                "depth": u32,
            )
        },
        |_task, _inputs| {
            Ok(outputs! {
                "color" => 10_u32,
                "depth" => 20_u32,
            })
        },
    )?;
    let lighting = graph.add_sync(
        "lighting",
        schema! {
            (
                "color": u32,
                "depth": u32,
            ) -> ("hdr": u32)
        },
        |_task, inputs| {
            let color = inputs.required::<u32>("color")?;
            let depth = inputs.required::<u32>("depth")?;
            Ok(outputs! { "hdr" => *color + *depth })
        },
    )?;

    let report = graph.connect_nodes(gbuffer, lighting)?;
    println!("auto-connected edges: {report:?}");
    graph.set_active(lighting, true)?;
    let version = graph.compile()?;

    let _report = futures_lite::future::block_on(version.execute(RunInputs::new()))?;
    Ok(())
}
