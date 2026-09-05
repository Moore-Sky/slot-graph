//! Scenario 27: a cohesive renderer-style pipeline with keyed task I/O.
//!
//! The host supplies one scene frame and one UI frame per run. Culling fans
//! out to two shadow passes and a G-buffer pass; lighting collects the shadow
//! contributions, then composition joins lighting with asynchronously prepared
//! UI. The IDs below are host-owned resource handles, not GPU objects.
//! Completing the graph does not mean GPU work has finished; allocation,
//! submission, fences, and resource retirement remain the renderer's job.
//!

use futures_lite::future::block_on;
use slot_graph::{Graph, InputSpec, Local, NodeOutputs, OutputSpec, RunInputs, Schema};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SceneFrame {
    frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UiFrame {
    frame: u64,
    root: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleSet(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextureId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GBuffer {
    albedo: TextureId,
    depth: TextureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShadowContribution {
    light: u8,
    texture: TextureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LitFrame {
    frame: u64,
    color: TextureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UiLayer(TextureId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FinalFrame {
    frame: u64,
    color: TextureId,
    ui: TextureId,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();

    // Bind names to compact, layout-scoped keys once. The closures below do no
    // name lookup: they use only their captured input and output keys.
    let cull_schema = Schema::builder()
        .input(InputSpec::required_one::<SceneFrame>("frame"))
        .output(OutputSpec::new::<VisibleSet>("visible"))
        .build()
        .bind();
    let cull_frame = cull_schema.input::<SceneFrame>("frame")?;
    let cull_visible = cull_schema.output::<VisibleSet>("visible")?;
    let cull = graph.add_sync("cull", cull_schema.clone(), move |_, inputs| {
        let frame = inputs.required_key(cull_frame)?;
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(cull_visible, VisibleSet(frame.frame));
        Ok(outputs)
    })?;

    let shadow_schema = Schema::builder()
        .input(InputSpec::required_one::<VisibleSet>("visible"))
        .output(OutputSpec::new::<ShadowContribution>("shadow"))
        .build()
        .bind();
    let shadow_visible = shadow_schema.input::<VisibleSet>("visible")?;
    let shadow_output = shadow_schema.output::<ShadowContribution>("shadow")?;
    let shadow_a = graph.add_sync("shadow_a", shadow_schema.clone(), move |_, inputs| {
        let visible = inputs.required_key(shadow_visible)?;
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(
            shadow_output,
            ShadowContribution {
                light: 0,
                texture: TextureId(100 + visible.0 as u32),
            },
        );
        Ok(outputs)
    })?;
    let shadow_b = graph.add_sync("shadow_b", shadow_schema.clone(), move |_, inputs| {
        let visible = inputs.required_key(shadow_visible)?;
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(
            shadow_output,
            ShadowContribution {
                light: 1,
                texture: TextureId(200 + visible.0 as u32),
            },
        );
        Ok(outputs)
    })?;

    let gbuffer_schema = Schema::builder()
        .input(InputSpec::required_one::<VisibleSet>("visible"))
        .output(OutputSpec::new::<GBuffer>("gbuffer"))
        .build()
        .bind();
    let gbuffer_visible = gbuffer_schema.input::<VisibleSet>("visible")?;
    let gbuffer_output = gbuffer_schema.output::<GBuffer>("gbuffer")?;
    let gbuffer = graph.add_sync("gbuffer", gbuffer_schema.clone(), move |_, inputs| {
        let visible = inputs.required_key(gbuffer_visible)?;
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(
            gbuffer_output,
            GBuffer {
                albedo: TextureId(300 + visible.0 as u32),
                depth: TextureId(400 + visible.0 as u32),
            },
        );
        Ok(outputs)
    })?;

    let lighting_schema = Schema::builder()
        .input(InputSpec::required_one::<GBuffer>("gbuffer"))
        .input(InputSpec::required_many::<ShadowContribution>("shadows"))
        .output(OutputSpec::new::<LitFrame>("lit"))
        .build()
        .bind();
    let lighting_gbuffer = lighting_schema.input::<GBuffer>("gbuffer")?;
    let lighting_shadows = lighting_schema.input::<ShadowContribution>("shadows")?;
    let lighting_output = lighting_schema.output::<LitFrame>("lit")?;
    let lighting = graph.add_sync("lighting", lighting_schema.clone(), move |_, inputs| {
        let gbuffer = inputs.required_key(lighting_gbuffer)?;
        let shadows = inputs.many_key(lighting_shadows)?;
        assert_eq!(shadows.len(), 2, "both shadow passes contribute");
        let mut outputs = NodeOutputs::new();
        let shadow_checksum = shadows
            .iter()
            .map(|shadow| shadow.texture.0 + u32::from(shadow.light))
            .sum::<u32>();
        outputs.insert_key(
            lighting_output,
            LitFrame {
                frame: gbuffer.albedo.0 as u64 - 300,
                color: TextureId(gbuffer.albedo.0 + gbuffer.depth.0 + shadow_checksum),
            },
        );
        Ok(outputs)
    })?;

    let ui_schema = Schema::builder()
        .input(InputSpec::required_one::<UiFrame>("ui_frame"))
        .output(OutputSpec::new::<UiLayer>("ui"))
        .build()
        .bind();
    let ui_frame = ui_schema.input::<UiFrame>("ui_frame")?;
    let ui_output = ui_schema.output::<UiLayer>("ui")?;
    let ui_prepare = graph.add_async(
        "ui_prepare",
        ui_schema.clone(),
        move |_, inputs| async move {
            let frame = inputs.required_key(ui_frame)?;
            futures_lite::future::yield_now().await;
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(
                ui_output,
                UiLayer(TextureId(500 + frame.root + frame.frame as u32)),
            );
            Ok(outputs)
        },
    )?;

    let composite_schema = Schema::builder()
        .input(InputSpec::required_one::<LitFrame>("lit"))
        .input(InputSpec::required_one::<UiLayer>("ui"))
        .output(OutputSpec::new::<FinalFrame>("final"))
        .build()
        .bind();
    let composite_lit = composite_schema.input::<LitFrame>("lit")?;
    let composite_ui = composite_schema.input::<UiLayer>("ui")?;
    let composite_output = composite_schema.output::<FinalFrame>("final")?;
    let composite = graph.add_sync("composite", composite_schema, move |_, inputs| {
        let lit = inputs.required_key(composite_lit)?;
        let ui = inputs.required_key(composite_ui)?;
        let mut outputs = NodeOutputs::new();
        outputs.insert_key(
            composite_output,
            FinalFrame {
                frame: lit.frame,
                color: lit.color,
                ui: ui.0,
            },
        );
        Ok(outputs)
    })?;

    // External inputs participate in normal dependency readiness. Outside a
    // task, graph handles remain convenient for selecting those bindings.
    let scene_input = graph.expose_input(graph.input::<SceneFrame>(cull, "frame")?)?;
    let ui_input = graph.expose_input(graph.input::<UiFrame>(ui_prepare, "ui_frame")?)?;

    graph.connect_nodes(cull, shadow_a)?;
    graph.connect_nodes(cull, shadow_b)?;
    graph.connect_nodes(cull, gbuffer)?;
    // `collect_into` selects both source nodes and this one Many input. It
    // appends each matching output in source/schema order without scanning
    // unrelated TextureId or ShadowContribution producers in the graph.
    let shadows_input = graph.input::<ShadowContribution>(lighting, "shadows")?;
    let shadow_edges = graph.collect_into([shadow_a, shadow_b], shadows_input)?;
    assert_eq!(shadow_edges.len(), 2);
    graph.connect_nodes(gbuffer, lighting)?;
    graph.connect_nodes(lighting, composite)?;
    graph.connect_nodes(ui_prepare, composite)?;
    graph.set_active(composite, true)?;

    let final_output = graph.output::<FinalFrame>(composite, "final")?;
    let version = graph.compile()?;
    let mut runner = version.runner();

    let mut first_inputs = RunInputs::new();
    first_inputs.insert(scene_input, SceneFrame { frame: 1 })?;
    first_inputs.insert(ui_input, UiFrame { frame: 1, root: 9 })?;
    let mut first = block_on(runner.execute(first_inputs))?;
    let first = first.take_output(final_output)?;
    assert_eq!(
        *first,
        FinalFrame {
            frame: 1,
            color: TextureId(1005),
            ui: TextureId(510),
        }
    );

    let mut second_inputs = RunInputs::new();
    second_inputs.insert(scene_input, SceneFrame { frame: 2 })?;
    second_inputs.insert(ui_input, UiFrame { frame: 2, root: 10 })?;
    let mut second = block_on(runner.execute(second_inputs))?;
    let second = second.take_output(final_output)?;
    assert_eq!(
        *second,
        FinalFrame {
            frame: 2,
            color: TextureId(1009),
            ui: TextureId(512),
        }
    );
    Ok(())
}
