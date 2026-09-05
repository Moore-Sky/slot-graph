//! Scenario 10: Local accepts `!Send` state; SendMode uses thread-safe tasks
//! and values.
//!
//! Both graphs have the same API shape. Their mode selects the type bounds.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use slot_graph::{outputs, schema, Graph, Local, RunInputs, SendMode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local_state = Rc::new(RefCell::new(0_u32));
    let mut local = Graph::<Local>::new();
    let state = Rc::clone(&local_state);
    let local_node = local.add_async(
        "local_prepare",
        schema! { () -> ("count": u32) },
        move |_task, _inputs| {
            let state = Rc::clone(&state);
            async move {
                *state.borrow_mut() += 1;
                Ok(outputs! { "count" => *state.borrow() })
            }
        },
    )?;
    local.set_active(local_node, true)?;
    let local_output = local.output::<u32>(local_node, "count")?;
    let local_version = local.compile()?;
    let local_report = futures_lite::future::block_on(local_version.execute(RunInputs::new()))?;
    println!(
        "Local count: {}",
        local_report.output(local_output)?.as_ref()
    );

    let send_count = Arc::new(AtomicUsize::new(0));
    let mut send = Graph::<SendMode>::new();
    let count = Arc::clone(&send_count);
    let send_node = send.add_async(
        "send_prepare",
        schema! { () -> ("count": usize) },
        move |_task, _inputs| {
            let count = Arc::clone(&count);
            async move {
                let next = count.fetch_add(1, Ordering::Relaxed) + 1;
                Ok(outputs! { "count" => next })
            }
        },
    )?;
    send.set_active(send_node, true)?;
    let send_output = send.output::<usize>(send_node, "count")?;
    let send_version = send.compile()?;
    let send_report = futures_lite::future::block_on(send_version.execute(RunInputs::new()))?;
    println!("Send count: {}", send_report.output(send_output)?.as_ref());

    Ok(())
}
