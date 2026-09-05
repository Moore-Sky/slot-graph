//! Scenario 06: retain an Active target output in the final run report.
//!
//! A typed output handle is acquired from the declaration graph before
//! compilation. The report owns the successful target value until the report is dropped
//! or the value is moved out with `take_output`. The current skeleton deliberately panics
//! when run.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

#[derive(Debug, PartialEq, Eq)]
struct FinalImage(u32);

fn main() {
    take_target_output();
}

fn take_target_output() {
    let mut graph = Graph::<Local>::new();

    let post = graph
        .add_sync(
            "post_process",
            schema! { () -> ("final": FinalImage) },
            |_task, _inputs| Ok(outputs! { "final" => FinalImage(7) }),
        )
        .unwrap();
    let final_output = graph.output::<FinalImage>(post, "final").unwrap();
    graph.set_active(post, true).unwrap();

    let version = graph.compile().unwrap();
    let mut report = futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
    let final_image = report.take_output(final_output).unwrap();
    assert_eq!(*final_image, FinalImage(7));
}
