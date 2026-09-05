//! Handle edit, compile, and startup errors, then execute the corrected graph.
//! The current API skeleton deliberately panics when graph operations are called.

use futures_lite::future::block_on;
use slot_graph::{
    outputs, schema, CompileErrorKind, EditErrorKind, Graph, Local, RunInputs, StartErrorKind,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let target = graph.add_sync(
        "target",
        schema! { ("value": u32) -> ("value": u32) },
        |_, inputs| Ok(outputs! { "value" => *inputs.required::<u32>("value")? }),
    )?;
    if let Err(error) = graph.rename_node(target, "") {
        assert_eq!(error.kind, EditErrorKind::InvalidNodeName);
        eprintln!("edit rejected: {error}; context: {:?}", error.context);
    }
    graph.set_active(target, true)?;
    match graph.compile() {
        Err(error) if error.kind == CompileErrorKind::MissingRequiredInput => {}
        _ => panic!("expected an unsatisfied input"),
    }
    let key = graph.expose_input::<u32>(target.input("value"))?;
    let output = graph.output::<u32>(target, "value")?;
    let version = graph.compile()?;
    match version.start(RunInputs::new()) {
        Err(error) if error.kind == StartErrorKind::MissingRunInput => {}
        _ => panic!("expected missing external value before any task starts"),
    }
    let mut inputs = RunInputs::new();
    inputs.insert(key, 42_u32)?;
    let report = block_on(version.execute(inputs))?;
    assert_eq!(**report.output(output)?, 42);
    Ok(())
}
