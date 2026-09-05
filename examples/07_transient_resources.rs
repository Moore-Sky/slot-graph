//! Scenario 07: transient renderer handles flow through Slots.
//!
//! Slot-graph owns only the handle values for a run. The renderer owns the
//! real allocation, command submission, GPU fence, and eventual pool return.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

#[derive(Debug, PartialEq, Eq)]
struct RenderTargetHandle(u32);

fn main() {
    pass_transient_handles();
}

fn pass_transient_handles() {
    let mut graph = Graph::<Local>::new();

    let gbuffer = graph
        .add_sync(
            "gbuffer",
            schema! {
                () -> (
                    "color": RenderTargetHandle,
                    "depth": RenderTargetHandle,
                )
            },
            |_task, _inputs| {
                Ok(outputs! {
                    "color" => RenderTargetHandle(1),
                    "depth" => RenderTargetHandle(2),
                })
            },
        )
        .unwrap();
    let lighting = graph
        .add_sync(
            "lighting",
            schema! {
                ("color": RenderTargetHandle, "depth": RenderTargetHandle)
                -> ("hdr": RenderTargetHandle)
            },
            |_task, inputs| {
                let color = inputs.required::<RenderTargetHandle>("color")?;
                let depth = inputs.required::<RenderTargetHandle>("depth")?;
                Ok(outputs! {
                    "hdr" => RenderTargetHandle(color.0 + depth.0)
                })
            },
        )
        .unwrap();

    graph
        .connect(gbuffer.output("color"), lighting.input("color"))
        .unwrap();
    graph
        .connect(gbuffer.output("depth"), lighting.input("depth"))
        .unwrap();
    graph.set_active(lighting, true).unwrap();

    let version = graph.compile().unwrap();
    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
}
