//! Drive a graph using a host-owned poll loop and standard-library Waker.
//! The current API skeleton deliberately panics when graph operations are called.

use slot_graph::{outputs, schema, Graph, Local, RunInputs};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
    thread,
};

struct ThreadWake {
    thread: thread::Thread,
    notified: AtomicBool,
}
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.notified.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

fn drive<F: Future>(future: F) -> F::Output {
    let wake = Arc::new(ThreadWake {
        thread: thread::current(),
        notified: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&wake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        // Check a retained notification before parking, including wakes during poll.
        while !wake.notified.swap(false, Ordering::AcqRel) {
            thread::park();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<Local>::new();
    let node = graph.add_async(
        "yield_once",
        schema! { () -> ("value": u32) },
        |_, _| async {
            futures_lite::future::yield_now().await;
            Ok(outputs! { "value" => 42_u32 })
        },
    )?;
    graph.set_active(node, true)?;
    let output = graph.output::<u32>(node, "value")?;
    let version = graph.compile()?;
    let report = drive(version.start(RunInputs::new())?)?;
    assert_eq!(**report.output(output)?, 42);
    Ok(())
}
