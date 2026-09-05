//! Scenario 14: nodes pull long-lived textures from the host resource system
//! instead of turning every texture into a Slot.

use std::{collections::HashMap, sync::Arc};

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

#[derive(Clone, Copy)]
struct MaterialHandle(u32);
#[derive(Clone, Copy)]
struct TextureHandle(u32);

struct Resources {
    textures: HashMap<u32, &'static str>,
}

impl Resources {
    fn texture(&self, handle: TextureHandle) -> Option<&'static str> {
        self.textures.get(&handle.0).copied()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let resources = Arc::new(Resources {
        textures: HashMap::from([(7, "albedo.png")]),
    });
    let mut graph = Graph::<Local>::new();

    let material = graph.add_sync(
        "material",
        schema! { () -> ("material": MaterialHandle) },
        |_task, _inputs| Ok(outputs! { "material" => MaterialHandle(7) }),
    )?;
    let resources_for_draw = Arc::clone(&resources);
    let draw = graph.add_sync(
        "draw",
        schema! { ("material": MaterialHandle) -> () },
        move |_task, inputs| {
            let material = inputs.required::<MaterialHandle>("material")?;
            let texture = resources_for_draw
                .texture(TextureHandle(material.0))
                .expect("the demo resource table contains the material texture");
            println!("draw with long-lived texture: {texture}");
            Ok(outputs! {})
        },
    )?;

    graph.connect(material.output("material"), draw.input("material"))?;
    graph.set_active(draw, true)?;
    let version = graph.compile()?;

    // Resources still owns the texture; GraphRun only carries this run's handle.
    let _report = futures_lite::future::block_on(version.execute(RunInputs::new()))?;
    Ok(())
}
