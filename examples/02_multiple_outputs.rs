//! Scenario 02: one node commits many outputs but creates one node dependency.
//!
//! `gbuffer` must commit color, depth, and normal atomically. The three Slot
//! edges must not make `lighting` ready three times.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() {
    multiple_outputs();
}

fn multiple_outputs() {
    let mut graph = Graph::<Local>::new();

    let gbuffer = graph
        .add_sync(
            "gbuffer",
            schema! {
                () -> (
                    "color": u32,
                    "depth": u32,
                    "normal": u32,
                )
            },
            |_task, _inputs| {
                Ok(outputs! {
                    "color" => 1_u32,
                    "depth" => 2_u32,
                    "normal" => 3_u32,
                })
            },
        )
        .unwrap();

    let lighting = graph
        .add_sync(
            "lighting",
            schema! { ("color": u32, "depth": u32, "normal": u32) -> () },
            |_task, inputs| {
                let color = inputs.required::<u32>("color")?;
                let depth = inputs.required::<u32>("depth")?;
                let normal = inputs.required::<u32>("normal")?;
                assert_eq!((*color, *depth, *normal), (1, 2, 3));
                Ok(outputs! {})
            },
        )
        .unwrap();

    for name in ["color", "depth", "normal"] {
        graph
            .connect(gbuffer.output(name), lighting.input(name))
            .unwrap();
    }
    graph.set_active(lighting, true).unwrap();

    let version = graph.compile().unwrap();
    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
}
