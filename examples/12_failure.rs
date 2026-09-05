//! Scenario 12: a failure blocks only its descendants; an independent active
//! target can still succeed and appear in the report.

use std::{error::Error, fmt};

use slot_graph::{outputs, schema, ExecuteError, Graph, Local, NodeError, RunInputs};

#[derive(Debug)]
struct DecodeError;

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("demo decode failed")
    }
}

impl Error for DecodeError {}

fn main() -> Result<(), Box<dyn Error>> {
    let mut graph = Graph::<Local>::new();
    let failed = graph.add_sync(
        "decode",
        schema! { () -> ("image": Vec<u8>) },
        |_task, _inputs| Err(NodeError::<Local>::user(DecodeError)),
    )?;
    let blocked = graph.add_sync(
        "upload",
        schema! { ("image": Vec<u8>) -> () },
        |_task, _inputs| Ok(outputs! {}),
    )?;
    let independent = graph.add_sync(
        "diagnostics",
        schema! { () -> ("message": String) },
        |_task, _inputs| Ok(outputs! { "message" => String::from("still ran") }),
    )?;

    graph.connect(failed.output("image"), blocked.input("image"))?;
    graph.set_active(blocked, true)?;
    graph.set_active(independent, true)?;
    let version = graph.compile()?;

    match futures_lite::future::block_on(version.execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => {
            for failure in report.failures() {
                println!("failed node {:?}: {}", failure.node, failure.error);
            }
            println!("upload status: {:?}", report.status(blocked));
            println!("diagnostics status: {:?}", report.status(independent));
        }
        Ok(_) => return Err("expected a Failed report".into()),
        Err(other) => return Err(format!("expected Failed report, got {other:?}").into()),
    }

    Ok(())
}
