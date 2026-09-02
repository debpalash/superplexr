//! Independently implemented, permissively licensed utility seam for GPUI.
//!
//! This crate intentionally implements only general-purpose interfaces required
//! by termi9ne's pinned GPUI dependency closure.

use std::{
    ffi::OsStr,
    future::Future,
    hash::{BuildHasher, DefaultHasher, Hasher},
    ops::AddAssign,
    panic::Location,
    task::{Context, Poll},
    time::Instant,
};

use pin_project_lite::pin_project;

pub mod arc_cow;

/// Creates a standard child-process command.
pub fn new_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    std::process::Command::new(program)
}

/// Returns the prior value and increments the supplied value by one.
pub fn post_inc<T: From<u8> + AddAssign<T> + Copy>(value: &mut T) -> T {
    let previous = *value;
    *value += T::from(1);
    previous
}

/// Executes a closure and emits its elapsed time at trace level.
pub fn measure<R>(label: &str, operation: impl FnOnce() -> R) -> R {
    let started_at = Instant::now();
    let result = operation();
    log::trace!("{label} took {:?}", started_at.elapsed());
    result
}

/// Panics in debug builds and logs the same message in release builds.
#[macro_export]
macro_rules! debug_panic {
    ($($argument:tt)*) => {{
        if cfg!(debug_assertions) {
            panic!($($argument)*);
        } else {
            log::error!($($argument)*);
        }
    }};
}

/// Evaluates a block inside a closure so `?` returns from that block.
#[macro_export]
macro_rules! maybe {
    ($body:block) => {{ (|| $body)() }};
}

/// Additional error-handling operations used throughout GPUI.
pub trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
    fn log_err_with_backtrace(self) -> Option<Self::Ok>
    where
        E: std::fmt::Debug;
    fn debug_assert_ok(self, reason: &str) -> Self;
    fn warn_on_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: log::Level) -> Option<Self::Ok>;
    fn anyhow(self) -> anyhow::Result<Self::Ok>
    where
        E: std::error::Error + Send + Sync + 'static;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: std::fmt::Display,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        self.log_with_level(log::Level::Error)
    }

    #[track_caller]
    fn log_err_with_backtrace(self) -> Option<T>
    where
        E: std::fmt::Debug,
    {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log::error!("{error:?}");
                None
            }
        }
    }

    #[track_caller]
    fn debug_assert_ok(self, reason: &str) -> Self {
        if cfg!(debug_assertions)
            && let Err(error) = &self
        {
            panic!("{reason}: {error}");
        }
        self
    }

    #[track_caller]
    fn warn_on_err(self) -> Option<T> {
        self.log_with_level(log::Level::Warn)
    }

    #[track_caller]
    fn log_with_level(self, level: log::Level) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log::log!(level, "{error}");
                None
            }
        }
    }

    fn anyhow(self) -> anyhow::Result<T>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.map_err(anyhow::Error::new)
    }
}

/// Logs a displayable error at error level and discards it.
pub fn log_err(error: &impl std::fmt::Display) {
    log::error!("{error}");
}

pin_project! {
    /// Future returned by [`TryFutureExt`] logging methods.
    pub struct LogErrorFuture<F> {
        #[pin]
        inner: F,
        level: log::Level,
        location: Location<'static>,
    }
}

pin_project! {
    /// Future returned by [`TryFutureExtBacktrace`] logging methods.
    pub struct LogErrorWithBacktraceFuture<F> {
        #[pin]
        inner: F,
        location: Location<'static>,
    }
}

pin_project! {
    /// Future that unwraps a fallible future's output.
    pub struct UnwrapFuture<F> {
        #[pin]
        inner: F,
    }
}

/// Logging and unwrapping adapters for fallible futures.
pub trait TryFutureExt<T, E>: Future<Output = Result<T, E>> + Sized
where
    E: std::fmt::Display,
{
    fn log_err(self) -> LogErrorFuture<Self>;
    fn log_tracked_err(self, location: Location<'static>) -> LogErrorFuture<Self>;
    fn warn_on_err(self) -> LogErrorFuture<Self>;
    fn unwrap(self) -> UnwrapFuture<Self>
    where
        E: std::fmt::Debug;
}

impl<F, T, E> TryFutureExt<T, E> for F
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    #[track_caller]
    fn log_err(self) -> LogErrorFuture<Self> {
        LogErrorFuture {
            inner: self,
            level: log::Level::Error,
            location: *Location::caller(),
        }
    }

    fn log_tracked_err(self, location: Location<'static>) -> LogErrorFuture<Self> {
        LogErrorFuture {
            inner: self,
            level: log::Level::Error,
            location,
        }
    }

    #[track_caller]
    fn warn_on_err(self) -> LogErrorFuture<Self> {
        LogErrorFuture {
            inner: self,
            level: log::Level::Warn,
            location: *Location::caller(),
        }
    }

    fn unwrap(self) -> UnwrapFuture<Self>
    where
        E: std::fmt::Debug,
    {
        UnwrapFuture { inner: self }
    }
}

impl<F, T, E> Future for LogErrorFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    type Output = Option<T>;

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let projected = self.project();
        match projected.inner.poll(context) {
            Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
            Poll::Ready(Err(error)) => {
                log::log!(
                    *projected.level,
                    "{}:{}: {error}",
                    projected.location.file(),
                    projected.location.line()
                );
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Backtrace-style logging adapters for fallible futures.
pub trait TryFutureExtBacktrace<T, E>: Future<Output = Result<T, E>> + Sized
where
    E: std::fmt::Debug,
{
    fn log_err_with_backtrace(self) -> LogErrorWithBacktraceFuture<Self>;
    fn log_tracked_err_with_backtrace(
        self,
        location: Location<'static>,
    ) -> LogErrorWithBacktraceFuture<Self>;
}

impl<F, T, E> TryFutureExtBacktrace<T, E> for F
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    #[track_caller]
    fn log_err_with_backtrace(self) -> LogErrorWithBacktraceFuture<Self> {
        Self::log_tracked_err_with_backtrace(self, *Location::caller())
    }

    fn log_tracked_err_with_backtrace(
        self,
        location: Location<'static>,
    ) -> LogErrorWithBacktraceFuture<Self> {
        LogErrorWithBacktraceFuture {
            inner: self,
            location,
        }
    }
}

impl<F, T, E> Future for LogErrorWithBacktraceFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    type Output = Option<T>;

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let projected = self.project();
        match projected.inner.poll(context) {
            Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
            Poll::Ready(Err(error)) => {
                log::error!(
                    "{}:{}: {error:?}",
                    projected.location.file(),
                    projected.location.line()
                );
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<F, T, E> Future for UnwrapFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().inner.poll(context) {
            Poll::Ready(result) => Poll::Ready(result.expect("fallible future failed")),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Runs a closure when this guard is dropped unless it is consumed.
pub struct Deferred<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Deferred<F> {
    /// Prevents the deferred callback from running.
    pub fn abort(mut self) {
        self.0.take();
    }
}

impl<F: FnOnce()> Drop for Deferred<F> {
    fn drop(&mut self) {
        if let Some(callback) = self.0.take() {
            callback();
        }
    }
}

/// Creates a scope guard that runs the callback when dropped.
pub fn defer<F: FnOnce()>(callback: F) -> Deferred<F> {
    Deferred(Some(callback))
}

/// Hash builder optimized by the standard library for hash-map use.
#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct TypeIdHashBuilder;

impl BuildHasher for TypeIdHashBuilder {
    type Hasher = TypeIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        TypeIdHasher(DefaultHasher::new())
    }
}

/// Safe hasher used for maps keyed by [`std::any::TypeId`].
pub struct TypeIdHasher(DefaultHasher);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::{ResultExt as _, defer, post_inc};

    #[test]
    fn deferred_callback_runs_once() {
        let mut count = 0;
        {
            let _guard = defer(|| count += 1);
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn result_and_increment_adapters_preserve_values() {
        assert_eq!(Ok::<_, &str>(9).log_err(), Some(9));
        let mut value = 4_u64;
        assert_eq!(post_inc(&mut value), 4);
        assert_eq!(value, 5);
    }
}
