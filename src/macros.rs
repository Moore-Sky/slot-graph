//! Declaration conveniences that expand to the ordinary public schema/value APIs.

/// Declares a Schema using `(inputs) -> (outputs)` syntax.
///
/// Input shorthand supports `T`, `Optional<T>`, `Many<T>`, and `Optional<Many<T>>`.
/// These are macro syntax, not wrapper types. For explicit Slot identities or
/// auto-collection, use the ordinary Schema/InputSpec builders.
///
/// ```
/// use slot_graph::{schema, Cardinality, Presence};
/// let schema = schema! { ("items": Optional<Many<u32>>) -> ("sum": u32) };
/// assert_eq!(schema.inputs[0].presence, Presence::Optional);
/// assert_eq!(schema.inputs[0].cardinality, Cardinality::Many);
/// ```
#[macro_export]
macro_rules! schema {
    (($($inputs:tt)*) -> ($($outputs:tt)*)) => {{
        let mut inputs = ::std::vec::Vec::new();
        $crate::schema!(@inputs inputs; $($inputs)*);
        let mut outputs = ::std::vec::Vec::new();
        $crate::schema!(@outputs outputs; $($outputs)*);
        $crate::Schema::new(inputs, outputs)
    }};
    (@inputs $vec:ident; ) => {};
    (@inputs $vec:ident; $name:literal : Optional<Many<$ty:ty>> $(, $($rest:tt)*)?) => {{ $vec.push($crate::InputSpec::optional_many::<$ty>($name)); $crate::schema!(@inputs $vec; $($($rest)*)?); }};
    (@inputs $vec:ident; $name:literal : Optional<$ty:ty> $(, $($rest:tt)*)?) => {{ $vec.push($crate::InputSpec::optional_one::<$ty>($name)); $crate::schema!(@inputs $vec; $($($rest)*)?); }};
    (@inputs $vec:ident; $name:literal : Many<$ty:ty> $(, $($rest:tt)*)?) => {{ $vec.push($crate::InputSpec::required_many::<$ty>($name)); $crate::schema!(@inputs $vec; $($($rest)*)?); }};
    (@inputs $vec:ident; $name:literal : $ty:ty $(, $($rest:tt)*)?) => {{ $vec.push($crate::InputSpec::required_one::<$ty>($name)); $crate::schema!(@inputs $vec; $($($rest)*)?); }};
    (@outputs $vec:ident; ) => {};
    (@outputs $vec:ident; $name:literal : $ty:ty $(, $($rest:tt)*)?) => {{ $vec.push($crate::OutputSpec::new::<$ty>($name)); $crate::schema!(@outputs $vec; $($($rest)*)?); }};
}

/// Builds the complete, uncommitted output bag returned by a task.
///
/// This is the named convenience path, not the lookup-free keyed path. Use
/// [`NodeOutputs::insert_key`][crate::NodeOutputs::insert_key] and
/// [`NodeOutputs::insert_shared_key`][crate::NodeOutputs::insert_shared_key]
/// with pre-bound output keys when names must be resolved before execution.
///
/// Duplicate, missing, unexpected, or incorrectly typed outputs are validated
/// together at commit time. Output storage remains unimplemented in this revision.
///
/// ```no_run
/// use slot_graph::{outputs, Local, NodeOutputs};
/// let outputs: NodeOutputs<Local> = outputs! { "answer" => 42_u32 };
/// ```
#[macro_export]
macro_rules! outputs {
    () => { $crate::NodeOutputs::new() };
    ($($name:literal => $value:expr),* $(,)?) => {{
        let mut outputs = $crate::NodeOutputs::new();
        $(outputs.insert($name, $value);)*
        outputs
    }};
}
