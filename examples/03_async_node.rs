//! Scenario 03: an async producer feeds a synchronous consumer.
//!
//! `futures_lite::future::block_on` only demonstrates how a host can drive an ordinary Future;
//! it is not a runtime dependency of slot-graph. The current skeleton panics
//! deliberately when run.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};

fn main() {
    async_then_sync();
}

fn async_then_sync() {
    let mut graph = Graph::<Local>::new();

    let load = graph
        .add_async(
            "load",
            schema! { () -> ("bytes": Vec<u8>) },
            |_task, _inputs| async move { Ok(outputs! { "bytes" => vec![1, 2, 3] }) },
        )
        .unwrap();

    let decode = graph
        .add_sync(
            "decode",
            schema! { ("bytes": Vec<u8>) -> () },
            |_task, inputs| {
                let bytes = inputs.required::<Vec<u8>>("bytes")?;
                assert_eq!(bytes.as_slice(), [1, 2, 3]);
                Ok(outputs! {})
            },
        )
        .unwrap();

    graph
        .connect(load.output("bytes"), decode.input("bytes"))
        .unwrap();
    graph.set_active(decode, true).unwrap();

    let version = graph.compile().unwrap();
    let _ = futures_lite::future::block_on(version.execute(RunInputs::new()));
}
