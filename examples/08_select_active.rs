//! Scenario 08: an Active target selects the reverse dependency closure.
//!
//! low and high are alternate endpoints. When only high is active, an
//! incomplete low branch does not participate in this version's compilation.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() {
    select_high_quality_pipeline();
}

fn select_high_quality_pipeline() {
    let mut graph = Graph::<Local>::new();

    let lighting = graph
        .add_sync(
            "lighting",
            schema! { () -> ("hdr": u32) },
            |_task, _inputs| Ok(outputs! { "hdr" => 100_u32 }),
        )
        .unwrap();
    let low = graph
        .add_sync(
            "low_post",
            schema! { ("hdr": u32) -> () },
            |_task, _inputs| Ok(outputs! {}),
        )
        .unwrap();
    let high = graph
        .add_sync(
            "high_post",
            schema! { ("hdr": u32) -> () },
            |_task, _inputs| Ok(outputs! {}),
        )
        .unwrap();

    graph
        .connect(lighting.output("hdr"), low.input("hdr"))
        .unwrap();
    graph
        .connect(lighting.output("hdr"), high.input("hdr"))
        .unwrap();

    graph.set_active(low, false).unwrap();
    graph.set_active(high, true).unwrap();

    let version = graph.compile().unwrap();
    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
}
