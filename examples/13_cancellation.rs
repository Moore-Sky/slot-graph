//! Scenario 13: obtain an independent RunControl and cooperatively cancel a
//! SendMode run.
//!
//! A task that finishes after cancellation does not publish new outputs. An
//! abort drops unfinished futures before the next poll; this example uses
//! cooperative cancellation instead.

use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

use slot_graph::{outputs, schema, Graph, RunInputs, SendMode};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<SendMode>::new();
    let wait = graph.add_async(
        "wait_for_cancel",
        schema! { () -> ("finished": bool) },
        |task, _inputs| async move {
            task.cancellation().cancelled().await;
            Ok(outputs! { "finished" => true })
        },
    )?;
    graph.set_active(wait, true)?;
    let version = graph.compile()?;

    let mut run = Box::pin(version.start(RunInputs::new())?);
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    // The first poll starts the task, which is then pending on CancellationToken.
    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    let control = run.as_ref().control();
    control.cancel();

    match futures_lite::future::block_on(run) {
        Ok(report) => println!("cancelled run status: {:?}", report.status(wait)),
        Err(error) => println!("cancelled run outcome: {error:?}"),
    }

    Ok(())
}
