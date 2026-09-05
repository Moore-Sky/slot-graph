#![allow(dead_code)]

use criterion::Criterion;
use slot_graph::{
    Graph, InputSpec, Local, NodeId, NodeOutputs, OutputSlot, OutputSpec, Schema, SendMode,
};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum IoStyle {
    Named,
    Bound,
}

#[derive(Clone, Copy)]
pub enum AsyncStyle {
    Ready,
    PendingOnce,
    WakeStorm,
    Mixed,
}

pub fn criterion_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10))
        .sample_size(100)
        .confidence_level(0.99)
        .significance_level(0.01)
}

pub fn empty_schema() -> Schema {
    Schema::builder().build()
}

pub fn source_schema() -> Schema {
    Schema::builder()
        .output(OutputSpec::new::<u64>("value"))
        .build()
}

pub fn step_schema() -> Schema {
    Schema::builder()
        .input(InputSpec::required_one::<u64>("value"))
        .output(OutputSpec::new::<u64>("value"))
        .build()
}

pub fn many_schema() -> Schema {
    Schema::builder()
        .input(InputSpec::required_many::<u64>("values"))
        .output(OutputSpec::new::<u64>("value"))
        .build()
}

pub fn build_local_chain(
    size: usize,
    active_nodes: usize,
    io: IoStyle,
) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(size > 0 && active_nodes > 0 && active_nodes <= size);
    let mut graph = Graph::<Local>::new();
    let mut nodes = Vec::with_capacity(size);

    for index in 0..size {
        let node = if index == 0 {
            match io {
                IoStyle::Named => {
                    graph.add_sync(format!("node_{index}"), source_schema(), |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", 1_u64);
                        Ok(outputs)
                    })
                }
                IoStyle::Bound => {
                    let schema = source_schema().bind();
                    let output = schema.output::<u64>("value").unwrap();
                    graph.add_sync(format!("node_{index}"), schema, move |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(output, 1_u64);
                        Ok(outputs)
                    })
                }
            }
        } else {
            match io {
                IoStyle::Named => {
                    graph.add_sync(format!("node_{index}"), step_schema(), |_, inputs| {
                        let value = inputs.required::<u64>("value")?;
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", *value + 1);
                        Ok(outputs)
                    })
                }
                IoStyle::Bound => {
                    let schema = step_schema().bind();
                    let input = schema.input::<u64>("value").unwrap();
                    let output = schema.output::<u64>("value").unwrap();
                    graph.add_sync(format!("node_{index}"), schema, move |_, inputs| {
                        let value = inputs.required_key(input)?;
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(output, *value + 1);
                        Ok(outputs)
                    })
                }
            }
        }
        .unwrap();
        if let Some(&previous) = nodes.last() {
            graph.connect_nodes(previous, node).unwrap();
        }
        nodes.push(node);
    }

    let target_index = active_nodes - 1;
    graph.set_active(nodes[target_index], true).unwrap();
    let output = graph.output::<u64>(nodes[target_index], "value").unwrap();
    (graph, output)
}

pub fn build_send_chain(size: usize, io: IoStyle) -> (Graph<SendMode>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<SendMode>::new();
    let mut nodes = Vec::with_capacity(size);
    for index in 0..size {
        let node = if index == 0 {
            match io {
                IoStyle::Named => {
                    graph.add_sync(format!("node_{index}"), source_schema(), |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", 1_u64);
                        Ok(outputs)
                    })
                }
                IoStyle::Bound => {
                    let schema = source_schema().bind();
                    let output = schema.output::<u64>("value").unwrap();
                    graph.add_sync(format!("node_{index}"), schema, move |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(output, 1_u64);
                        Ok(outputs)
                    })
                }
            }
        } else {
            match io {
                IoStyle::Named => {
                    graph.add_sync(format!("node_{index}"), step_schema(), |_, inputs| {
                        let value = inputs.required::<u64>("value")?;
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", *value + 1);
                        Ok(outputs)
                    })
                }
                IoStyle::Bound => {
                    let schema = step_schema().bind();
                    let input = schema.input::<u64>("value").unwrap();
                    let output = schema.output::<u64>("value").unwrap();
                    graph.add_sync(format!("node_{index}"), schema, move |_, inputs| {
                        let value = inputs.required_key(input)?;
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(output, *value + 1);
                        Ok(outputs)
                    })
                }
            }
        }
        .unwrap();
        if let Some(&previous) = nodes.last() {
            graph.connect_nodes(previous, node).unwrap();
        }
        nodes.push(node);
    }
    let target = *nodes.last().unwrap();
    graph.set_active(target, true).unwrap();
    let output = graph.output::<u64>(target, "value").unwrap();
    (graph, output)
}

pub fn build_send_independent(size: usize) -> (Graph<SendMode>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<SendMode>::new();
    let schema = source_schema().bind();
    let output_key = schema.output::<u64>("value").unwrap();
    let mut selected = None;
    for index in 0..size {
        let node = graph
            .add_sync(format!("node_{index}"), schema.clone(), move |_, _| {
                let mut outputs = NodeOutputs::new();
                outputs.insert_key(output_key, index as u64);
                Ok(outputs)
            })
            .unwrap();
        graph.set_active(node, true).unwrap();
        selected = Some(node);
    }
    let output = graph.output::<u64>(selected.unwrap(), "value").unwrap();
    (graph, output)
}

pub fn build_local_renderer_frame(width: usize, layers: usize) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(width > 0 && layers > 0);
    let mut graph = Graph::<Local>::new();
    let source = source_schema().bind();
    let source_output = source.output::<u64>("value").unwrap();
    let mut previous = Vec::with_capacity(width);
    for index in 0..width {
        previous.push(
            graph
                .add_sync(
                    format!("layer_0_node_{index}"),
                    source.clone(),
                    move |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(source_output, index as u64 + 1);
                        Ok(outputs)
                    },
                )
                .unwrap(),
        );
    }

    for layer in 1..layers {
        let mut current = Vec::with_capacity(width);
        for index in 0..width {
            let schema = many_schema().bind();
            let input = schema.input::<u64>("values").unwrap();
            let output = schema.output::<u64>("value").unwrap();
            let node = graph
                .add_sync(
                    format!("layer_{layer}_node_{index}"),
                    schema,
                    move |_, inputs| {
                        let value = inputs
                            .many_key(input)?
                            .iter()
                            .fold(index as u64, |sum, value| sum.wrapping_add(**value));
                        let mut outputs = NodeOutputs::new();
                        outputs.insert_key(output, value);
                        Ok(outputs)
                    },
                )
                .unwrap();
            let target = graph.input::<u64>(node, "values").unwrap();
            graph
                .collect_into(previous.iter().copied(), target)
                .unwrap();
            current.push(node);
        }
        previous = current;
    }

    for &node in &previous {
        graph.set_active(node, true).unwrap();
    }
    let output = graph
        .output::<u64>(*previous.last().unwrap(), "value")
        .unwrap();
    (graph, output)
}

pub fn renderer_expected(width: usize, layers: usize) -> u64 {
    let mut values: Vec<u64> = (1..=width as u64).collect();
    for _ in 1..layers {
        let sum = values.iter().copied().fold(0_u64, u64::wrapping_add);
        values = (0..width)
            .map(|index| sum.wrapping_add(index as u64))
            .collect();
    }
    *values.last().unwrap()
}

pub fn build_local_async_chain(size: usize, style: AsyncStyle) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<Local>::new();
    let mut nodes = Vec::with_capacity(size);
    for index in 0..size {
        let schema = if index == 0 {
            source_schema()
        } else {
            step_schema()
        }
        .bind();
        let input = (index > 0).then(|| schema.input::<u64>("value").unwrap());
        let output = schema.output::<u64>("value").unwrap();
        let node_style = if matches!(style, AsyncStyle::Mixed) && index % 5 != 0 {
            None
        } else {
            Some(style)
        };
        let node = match node_style {
            None => graph.add_sync(format!("node_{index}"), schema, move |_, inputs| {
                let value = match input {
                    Some(key) => *inputs.required_key(key)? + 1,
                    None => 1,
                };
                let mut outputs = NodeOutputs::new();
                outputs.insert_key(output, value);
                Ok(outputs)
            }),
            Some(node_style) => graph.add_async(
                format!("node_{index}"),
                schema,
                move |_, inputs| async move {
                    match node_style {
                        AsyncStyle::Ready | AsyncStyle::Mixed => {}
                        AsyncStyle::PendingOnce => YieldCount::pending_once().await,
                        AsyncStyle::WakeStorm => YieldCount::wake_storm(8).await,
                    }
                    let value = match input {
                        Some(key) => *inputs.required_key(key)? + 1,
                        None => 1,
                    };
                    let mut outputs = NodeOutputs::new();
                    outputs.insert_key(output, value);
                    Ok(outputs)
                },
            ),
        }
        .unwrap();
        if let Some(&previous) = nodes.last() {
            graph.connect_nodes(previous, node).unwrap();
        }
        nodes.push(node);
    }
    let target = *nodes.last().unwrap();
    graph.set_active(target, true).unwrap();
    let output = graph.output::<u64>(target, "value").unwrap();
    (graph, output)
}

pub fn build_local_async_independent(
    size: usize,
    style: AsyncStyle,
) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<Local>::new();
    let schema = source_schema().bind();
    let output_key = schema.output::<u64>("value").unwrap();
    let mut selected = None;
    for index in 0..size {
        let node = graph
            .add_async(
                format!("node_{index}"),
                schema.clone(),
                move |_, _| async move {
                    match style {
                        AsyncStyle::Ready | AsyncStyle::Mixed => {}
                        AsyncStyle::PendingOnce => YieldCount::pending_once().await,
                        AsyncStyle::WakeStorm => YieldCount::wake_storm(8).await,
                    }
                    let mut outputs = NodeOutputs::new();
                    outputs.insert_key(output_key, index as u64);
                    Ok(outputs)
                },
            )
            .unwrap();
        graph.set_active(node, true).unwrap();
        selected = Some(node);
    }
    let output = graph.output::<u64>(selected.unwrap(), "value").unwrap();
    (graph, output)
}

pub fn build_send_async_chain(
    size: usize,
    style: AsyncStyle,
) -> (Graph<SendMode>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<SendMode>::new();
    let mut nodes = Vec::with_capacity(size);
    for index in 0..size {
        let schema = if index == 0 {
            source_schema()
        } else {
            step_schema()
        }
        .bind();
        let input = (index > 0).then(|| schema.input::<u64>("value").unwrap());
        let output = schema.output::<u64>("value").unwrap();
        let is_sync = matches!(style, AsyncStyle::Mixed) && index % 5 != 0;
        let node = if is_sync {
            graph.add_sync(format!("node_{index}"), schema, move |_, inputs| {
                let value = match input {
                    Some(key) => *inputs.required_key(key)? + 1,
                    None => 1,
                };
                let mut outputs = NodeOutputs::new();
                outputs.insert_key(output, value);
                Ok(outputs)
            })
        } else {
            graph.add_async(
                format!("node_{index}"),
                schema,
                move |_, inputs| async move {
                    match style {
                        AsyncStyle::Ready | AsyncStyle::Mixed => {}
                        AsyncStyle::PendingOnce => YieldCount::pending_once().await,
                        AsyncStyle::WakeStorm => YieldCount::wake_storm(8).await,
                    }
                    let value = match input {
                        Some(key) => *inputs.required_key(key)? + 1,
                        None => 1,
                    };
                    let mut outputs = NodeOutputs::new();
                    outputs.insert_key(output, value);
                    Ok(outputs)
                },
            )
        }
        .unwrap();
        if let Some(&previous) = nodes.last() {
            graph.connect_nodes(previous, node).unwrap();
        }
        nodes.push(node);
    }
    let target = *nodes.last().unwrap();
    graph.set_active(target, true).unwrap();
    let output = graph.output::<u64>(target, "value").unwrap();
    (graph, output)
}

pub fn build_send_async_independent(
    size: usize,
    style: AsyncStyle,
) -> (Graph<SendMode>, OutputSlot<u64>) {
    assert!(size > 0);
    let mut graph = Graph::<SendMode>::new();
    let schema = source_schema().bind();
    let output_key = schema.output::<u64>("value").unwrap();
    let mut selected = None;
    for index in 0..size {
        let node = graph
            .add_async(
                format!("node_{index}"),
                schema.clone(),
                move |_, _| async move {
                    match style {
                        AsyncStyle::Ready | AsyncStyle::Mixed => {}
                        AsyncStyle::PendingOnce => YieldCount::pending_once().await,
                        AsyncStyle::WakeStorm => YieldCount::wake_storm(8).await,
                    }
                    let mut outputs = NodeOutputs::new();
                    outputs.insert_key(output_key, index as u64);
                    Ok(outputs)
                },
            )
            .unwrap();
        graph.set_active(node, true).unwrap();
        selected = Some(node);
    }
    let output = graph.output::<u64>(selected.unwrap(), "value").unwrap();
    (graph, output)
}

pub fn add_local_producer(graph: &mut Graph<Local>, name: String, value: u64) -> NodeId {
    graph
        .add_sync(name, source_schema(), move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert("value", value);
            Ok(outputs)
        })
        .unwrap()
}

pub struct YieldCount {
    remaining: usize,
    wakes_per_poll: usize,
}

impl YieldCount {
    fn pending_once() -> Self {
        Self {
            remaining: 1,
            wakes_per_poll: 1,
        }
    }

    fn wake_storm(wakes_per_poll: usize) -> Self {
        Self {
            remaining: 1,
            wakes_per_poll,
        }
    }
}

impl Future for YieldCount {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.remaining == 0 {
            Poll::Ready(())
        } else {
            self.remaining -= 1;
            for _ in 0..self.wakes_per_poll {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }
    }
}
