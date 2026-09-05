//! Explicitly collect a texture inventory from selected producer nodes.
//!
//! `collect_into` adds ordinary edges only for the requested `Many` input. It
//! takes selected sources rather than searching the graph, collects every
//! exact-type output in caller/source-schema order, and is idempotent. Graph
//! execution deliberately panics in the current API skeleton. A renderer would
//! normally collect semantic values such as `ShadowContribution`, not every
//! raw texture; this example intentionally models a resource inventory.

use futures_lite::future::block_on;
use slot_graph::{outputs, schema, Graph, InputSpec, Local, RunInputs, Schema};

#[derive(Debug)]
struct Texture {
    id: u32,
}

#[derive(Debug)]
struct Camera {
    id: u32,
}

#[derive(Debug)]
struct RenderConfig {
    samples: u32,
}

#[derive(Debug, PartialEq)]
struct Inventory {
    texture_ids: Vec<u32>,
    camera_id: u32,
    samples: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let gbuffer = graph.add_sync(
        "gbuffer",
        schema! { () -> ("albedo": Texture, "normal": Texture, "camera": Camera, "label": String) },
        |_, _| {
            Ok(outputs! {
                "albedo" => Texture { id: 10 },
                "normal" => Texture { id: 11 },
                "camera" => Camera { id: 7 },
                "label" => String::from("opaque"),
            })
        },
    )?;
    let shadows = graph.add_sync(
        "shadows",
        schema! { () -> ("sun": Texture, "config": RenderConfig, "metadata": u32) },
        |_, _| {
            Ok(outputs! {
                "sun" => Texture { id: 20 },
                "config" => RenderConfig { samples: 4 },
                "metadata" => 1_u32,
            })
        },
    )?;
    let frame_schema = Schema::new(
        vec![
            InputSpec::required_many::<Texture>("textures"),
            InputSpec::required_one::<Camera>("camera"),
            InputSpec::required_one::<RenderConfig>("config"),
        ],
        vec![slot_graph::OutputSpec::new::<Inventory>("inventory")],
    );
    let composite = graph.add_sync("composite", frame_schema, |_, inputs| {
        let textures = inputs.many::<Texture>("textures")?;
        let camera = inputs.required::<Camera>("camera")?;
        let config = inputs.required::<RenderConfig>("config")?;
        Ok(outputs! { "inventory" => Inventory {
            texture_ids: textures.iter().map(|texture| texture.id).collect::<Vec<_>>(),
            camera_id: camera.id,
            samples: config.samples,
        }})
    })?;

    let created = graph.collect_into([gbuffer, shadows], composite.input("textures"))?;
    assert_eq!(created.len(), 3, "String and u32 outputs are not collected");
    assert!(graph
        .collect_into([gbuffer, shadows], composite.input("textures"))?
        .is_empty());
    // These ordinary edges still succeed after collection: only `textures` was
    // considered, never the other target inputs.
    graph.connect(gbuffer.output("camera"), composite.input("camera"))?;
    graph.connect(shadows.output("config"), composite.input("config"))?;

    graph.set_active(composite, true)?;
    let inventory = graph.output::<Inventory>(composite, "inventory")?;
    let version = graph.compile()?;
    let report = block_on(version.execute(RunInputs::new()))?;
    assert_eq!(
        **report.output(inventory)?,
        Inventory {
            texture_ids: vec![10, 11, 20],
            camera_id: 7,
            samples: 4,
        }
    );
    Ok(())
}
