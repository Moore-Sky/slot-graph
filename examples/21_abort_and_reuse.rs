//! Scenario 21: abort a pending frame and reuse the same runner for the next frame.

use futures_lite::future::block_on;
use slot_graph::{outputs, schema, ExecuteError, Graph, Local, RunInputs};
use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = Rc::new(Cell::new(false));
    let signal = Rc::clone(&started);
    let mut graph = Graph::<Local>::new();
    let node = graph.add_async(
        "frame",
        schema! { ("wait": bool) -> ("done": bool) },
        move |_, inputs| {
            let signal = Rc::clone(&signal);
            async move {
                signal.set(true);
                if *inputs.required::<bool>("wait")? {
                    std::future::pending::<()>().await;
                }
                Ok(outputs! { "done" => true })
            }
        },
    )?;
    let wait = graph.expose_input::<bool>(node.input("wait"))?;
    let done = graph.output::<bool>(node, "done")?;
    graph.set_active(node, true)?;
    let version = graph.compile()?;
    let mut runner = version.runner();
    let mut first_inputs = RunInputs::new();
    first_inputs.insert(wait, true)?;
    let mut first = Box::pin(runner.start(first_inputs)?);
    let control = first.control();
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    for _ in 0..32 {
        assert!(matches!(first.as_mut().poll(&mut context), Poll::Pending));
        if started.get() {
            break;
        }
    }
    assert!(started.get());
    control.abort();
    assert!(matches!(block_on(first), Err(ExecuteError::Cancelled(_))));

    let mut next_inputs = RunInputs::new();
    next_inputs.insert(wait, false)?;
    let report = block_on(runner.execute(next_inputs))?;
    assert!(**report.output(done)?);
    runner.trim();
    Ok(())
}
