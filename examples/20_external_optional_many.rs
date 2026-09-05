//! Scenario 20: supply an ordered external collection and an optional label per request.
use futures_lite::future::block_on;
use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let node = graph.add_sync("merge",
        schema! { ("label": Optional<String>, "items": Optional<Many<u32>>) -> ("summary": String) },
        |_, inputs| {
            let label = inputs.optional::<String>("label")?;
            let label = label.as_ref().map(|v| v.as_str()).unwrap_or("untitled");
            let items = inputs.many::<u32>("items")?;
            let total: u32 = items.iter().map(|v| **v).sum();
            Ok(outputs! { "summary" => format!("{label}: {total}") })
        })?;
    let label = graph.expose_input::<String>(node.input("label"))?;
    let items = graph.expose_input::<u32>(node.input("items"))?;
    let output = graph.output::<String>(node, "summary")?;
    graph.set_active(node, true)?;
    let version = graph.compile()?;
    let mut values = RunInputs::new();
    values.insert(label, String::from("frame"))?;
    values.extend(items, [1_u32, 2, 3])?;
    let report = block_on(version.execute(values))?;
    assert_eq!(report.output(output)?.as_str(), "frame: 6");

    // Both exposed Optional inputs may be omitted on the next run.
    let empty = block_on(version.execute(RunInputs::new()))?;
    assert_eq!(empty.output(output)?.as_str(), "untitled: 0");
    Ok(())
}
