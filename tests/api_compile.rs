//! Compile-time public API contracts.
//!
//! Keep this file free of an executor: these tests deliberately exercise type
//! checking and trait bounds only.  Rejections belong in future `trybuild`
//! tests once the public API is no longer moving.

use std::rc::Rc;

use slot_graph::{
    outputs, schema, BoundSchema, DispatchError, ExecutionGraphVersion, Graph, GraphRun, InputKey,
    InputSlot, Local, NodeDispatcher, NodeJob, OutputKey, OutputSlot, RunControl, RunInputs,
    SendMode, Shared,
};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn assert_copy<T: Copy>() {}

#[derive(Debug, PartialEq, Eq)]
struct NotClone(u32);

#[test]
fn send_public_values_have_the_documented_mobility() {
    assert_send::<ExecutionGraphVersion<SendMode>>();
    assert_sync::<ExecutionGraphVersion<SendMode>>();
    assert_send::<GraphRun<SendMode>>();
    assert_send::<RunControl<SendMode>>();
    assert_sync::<RunControl<SendMode>>();
    assert_send::<Shared<u32, SendMode>>();
    assert_sync::<Shared<u32, SendMode>>();
    assert_send::<NodeJob<SendMode>>();
}

// These bodies deliberately do not execute a graph. They make the external
// dispatch boundary part of the checked public contract: Send jobs can cross
// workers, while Local dispatchers may capture thread-affine state.
#[allow(dead_code)]
fn dispatchers_preserve_mode_bounds() {
    fn accepts_send<D: NodeDispatcher<SendMode> + Send + Sync>(_: D) {}
    fn accepts_local<D: NodeDispatcher<Local>>(_: D) {}

    accepts_send(|_job: NodeJob<SendMode>| Ok::<_, DispatchError>(()));

    let local_state = Rc::new(std::cell::Cell::new(0_u32));
    accepts_local(move |_job: NodeJob<Local>| {
        local_state.set(local_state.get() + 1);
        Ok::<_, DispatchError>(())
    });
}

#[test]
fn dispatch_error_retains_its_thread_safe_source() {
    use std::error::Error;

    let error = DispatchError::with_source(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "worker pool closed",
    ));
    assert_send::<DispatchError>();
    assert_sync::<DispatchError>();
    assert_eq!(error.to_string(), "worker pool closed");
    assert_eq!(error.source().unwrap().to_string(), "worker pool closed");
}

#[test]
fn shared_clone_does_not_require_the_value_to_be_clone() {
    let original = Shared::<NotClone, Local>::new(NotClone(7));
    let clone = original.clone();

    assert_eq!(original.as_ref().0, 7);
    assert_eq!(clone.as_ref().0, 7);
}

#[test]
fn shared_drops_the_value_after_its_last_owner() {
    struct DropProbe(Rc<Cell>);
    struct Cell(std::cell::Cell<usize>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0 .0.set(self.0 .0.get() + 1);
        }
    }

    let drops = Rc::new(Cell(std::cell::Cell::new(0)));
    let first = Shared::<DropProbe, Local>::new(DropProbe(Rc::clone(&drops)));
    let second = first.clone();
    drop(first);
    assert_eq!(drops.0.get(), 0);
    drop(second);
    assert_eq!(drops.0.get(), 1);
}

#[test]
fn typed_slot_handles_do_not_require_the_value_to_be_copy() {
    assert_copy::<InputSlot<NotClone>>();
    assert_copy::<OutputSlot<NotClone>>();
    assert_copy::<InputKey<NotClone>>();
    assert_copy::<OutputKey<NotClone>>();
    assert_copy::<slot_graph::RunInput<NotClone, Local>>();
}

// These functions are not called. Their bodies are compile contracts for the
// bound-layout fast path: a key is Copy even when its payload is not, and the
// Local/Send closure bounds stay equivalent to add_sync/add_async.
#[allow(dead_code)]
fn keyed_local_tasks_accept_non_send_payloads() {
    let bound: BoundSchema = schema! { ("value": NotClone) -> ("out": NotClone) }.bind();
    let input = bound.input::<NotClone>("value").unwrap();
    let output = bound.output::<NotClone>("out").unwrap();
    let mut graph = Graph::<Local>::new();
    graph
        .add_sync("keyed", bound.clone(), move |_, inputs| {
            let value = inputs.required_key(input)?;
            let mut outputs = slot_graph::NodeOutputs::new();
            outputs.insert_key(output, NotClone(value.0));
            Ok::<_, slot_graph::NodeError<Local>>(outputs)
        })
        .unwrap();
    graph
        .add_async("keyed_async", bound, move |_, inputs| async move {
            let value = inputs.required_key(input)?;
            let mut outputs = slot_graph::NodeOutputs::new();
            outputs.insert_key(output, NotClone(value.0));
            Ok::<_, slot_graph::NodeError<Local>>(outputs)
        })
        .unwrap();
}

#[allow(dead_code)]
fn keyed_send_tasks_keep_send_bounds() {
    let bound = schema! { ("value": u32) -> ("out": u32) }.bind();
    let input = bound.input::<u32>("value").unwrap();
    let output = bound.output::<u32>("out").unwrap();
    let mut graph = Graph::<SendMode>::new();
    graph
        .add_sync("keyed", bound.clone(), move |_, inputs| {
            let mut outputs = slot_graph::NodeOutputs::new();
            outputs.insert_key(output, *inputs.required_key(input)?);
            Ok::<_, slot_graph::NodeError<SendMode>>(outputs)
        })
        .unwrap();
    graph
        .add_async("keyed_async", bound, move |_, inputs| async move {
            let mut outputs = slot_graph::NodeOutputs::new();
            outputs.insert_key(output, *inputs.required_key(input)?);
            Ok::<_, slot_graph::NodeError<SendMode>>(outputs)
        })
        .unwrap();
}

#[test]
fn user_error_keeps_its_source_and_send_mode_error_is_send() {
    use std::error::Error;
    let error = slot_graph::NodeError::<SendMode>::user(std::io::Error::new(
        std::io::ErrorKind::Other,
        "asset unavailable",
    ));
    assert_send::<slot_graph::NodeError<SendMode>>();
    assert_send::<slot_graph::RunReport<SendMode>>();
    assert_eq!(error.source().unwrap().to_string(), "asset unavailable");
    assert!(error
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .is_some());
}

// This function is intentionally not called.  Its body is nevertheless
// type-checked, so it is a compact compile contract for Local + Rc + !Send
// task state without making a runtime scheduling claim.
#[allow(dead_code)]
fn local_accepts_rc_in_a_task_factory() {
    let state = Rc::new(41_u32);
    let captured = Rc::clone(&state);
    let mut graph = Graph::<Local>::new();

    graph
        .add_sync(
            "local_rc",
            schema! { () -> ("value": u32) },
            move |_task, _inputs| {
                Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => *captured })
            },
        )
        .unwrap();

    // Also type-check the public start boundary for the Local mode.
    let _run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
}

#[allow(dead_code)]
fn local_async_task_can_hold_rc_across_await() {
    let state = Rc::new(9_u32);
    let captured = Rc::clone(&state);
    let mut graph = Graph::<Local>::new();

    graph
        .add_async(
            "local_async_rc",
            schema! { () -> ("value": u32) },
            move |_task, _inputs| {
                let captured = Rc::clone(&captured);
                async move {
                    std::future::ready(()).await;
                    Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => *captured })
                }
            },
        )
        .unwrap();
}
