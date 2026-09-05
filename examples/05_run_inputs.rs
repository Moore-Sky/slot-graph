//! Scenario 05: a graph exposes an input that is supplied per run.
//!
//! The exposed `scene` source is validated before any task starts. It is not a
//! second scheduler or an ordinary producer edge. The current skeleton
//! deliberately panics when run.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

#[derive(Debug)]
struct Scene {
    object_count: u32,
}

fn main() {
    supply_run_input();
}

fn supply_run_input() {
    let mut graph = Graph::<Local>::new();

    let cull = graph
        .add_sync(
            "cull",
            schema! { ("scene": Scene) -> ("visible": u32) },
            |_task, inputs| {
                let scene = inputs.required::<Scene>("scene")?;
                Ok(outputs! { "visible" => scene.object_count })
            },
        )
        .unwrap();

    let scene_input = graph.expose_input::<Scene>(cull.input("scene")).unwrap();
    graph.set_active(cull, true).unwrap();

    let version = graph.compile().unwrap();
    let mut inputs = RunInputs::<Local>::new();
    inputs
        .insert(scene_input, Scene { object_count: 17 })
        .unwrap();
    let _ = futures_lite::future::block_on(version.execute(inputs));
}
