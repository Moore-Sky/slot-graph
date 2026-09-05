//! Execution modes and the value/error bounds accepted by each mode.

use std::{any::Any, error::Error, marker::PhantomData, rc::Rc};

mod sealed {
    pub trait Sealed {}
    pub trait Value<M> {}
    impl<T: 'static> Value<super::Local> for T {}
    impl<T: Send + Sync + 'static> Value<super::SendMode> for T {}
}

/// Execution-mode marker.  This trait is sealed; applications select either
/// [`Local`] or [`SendMode`] and do not implement it.
pub trait Mode: sealed::Sealed + 'static {
    /// Stores a user-supplied task error with this mode's thread-safety bound.
    #[doc(hidden)]
    type UserError: 'static;
    /// Views stored user error data through the standard error trait.
    #[doc(hidden)]
    fn user_error_ref(error: &Self::UserError) -> &(dyn Error + 'static);
}

/// A graph whose values and futures may be thread-local.
pub struct Local(PhantomData<Rc<()>>);
/// A graph whose factories, values and futures can cross thread boundaries.
pub struct SendMode;

impl sealed::Sealed for Local {}
impl sealed::Sealed for SendMode {}
impl Mode for Local {
    type UserError = Box<dyn Error + 'static>;
    fn user_error_ref(error: &Self::UserError) -> &(dyn Error + 'static) {
        error.as_ref()
    }
}
impl Mode for SendMode {
    type UserError = Box<dyn Error + Send + Sync + 'static>;
    fn user_error_ref(error: &Self::UserError) -> &(dyn Error + 'static) {
        error.as_ref()
    }
}

/// Converts an application error into the mode-specific error storage.
#[doc(hidden)]
pub trait UserErrorFor<M: Mode>: Error + 'static {
    /// Converts this error into the storage selected by `M`.
    fn into_user_error(self) -> M::UserError;
}
impl<E: Error + 'static> UserErrorFor<Local> for E {
    fn into_user_error(self) -> <Local as Mode>::UserError {
        Box::new(self)
    }
}
impl<E: Error + Send + Sync + 'static> UserErrorFor<SendMode> for E {
    fn into_user_error(self) -> <SendMode as Mode>::UserError {
        Box::new(self)
    }
}

/// A type which may be placed in a slot for execution mode `M`.
pub trait ValueFor<M: Mode>: sealed::Value<M> + Any + 'static {}
impl<T: Any + 'static> ValueFor<Local> for T {}
impl<T: Any + Send + Sync + 'static> ValueFor<SendMode> for T {}
