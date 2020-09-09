use std::cell::UnsafeCell;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// An async `unordered` mutex.
/// It will be works with any async runtime in `Rust`, it may be a `tokio`, `smol`, `async-std` and etc..
///
/// The main difference with the standard `Mutex` is unordered mutex will not check an ordering of blocking.
/// This way is much faster, but there are some risks what someone mutex lock will be executed much later.
pub struct UnorderedMutex<T: ?Sized> {
    is_acquired: AtomicBool,
    waker: AtomicPtr<Waker>,
    data: UnsafeCell<T>,
}

impl<T> UnorderedMutex<T> {
    /// Create a new `UnorderedMutex`
    #[inline]
    pub const fn new(data: T) -> UnorderedMutex<T> {
        UnorderedMutex {
            is_acquired: AtomicBool::new(false),
            waker: AtomicPtr::new(null_mut()),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> UnorderedMutex<T> {
    /// Acquires the mutex.
    ///
    /// Returns a guard that releases the mutex and wake the next locker when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use fast_async_mutex::mutex_unordered::UnorderedMutex;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mutex = UnorderedMutex::new(10);
    ///     let guard = mutex.lock().await;
    ///     assert_eq!(*guard, 10);
    /// }
    /// ```
    #[inline]
    pub const fn lock(&self) -> UnorderedMutexGuardFuture<T> {
        UnorderedMutexGuardFuture {
            mutex: &self,
            is_realized: false,
        }
    }

    /// Acquires the mutex.
    ///
    /// Returns a guard that releases the mutex and wake the next locker when dropped.
    /// `UnorderedMutexOwnedGuardFuture` have a `'static` lifetime, but requires the `Arc<Mutex<T>>` type
    ///
    /// # Examples
    ///
    /// ```
    /// use fast_async_mutex::mutex_unordered::UnorderedMutex;
    /// use std::sync::Arc;
    /// #[tokio::main]
    /// async fn main() {
    ///     let mutex = Arc::new(UnorderedMutex::new(10));
    ///     let guard = mutex.lock_owned().await;
    ///     assert_eq!(*guard, 10);
    /// }
    /// ```
    #[inline]
    pub fn lock_owned(self: &Arc<Self>) -> UnorderedMutexOwnedGuardFuture<T> {
        UnorderedMutexOwnedGuardFuture {
            mutex: self.clone(),
            is_realized: false,
        }
    }

    #[inline]
    fn unlock(&self) {
        self.is_acquired.store(false, Ordering::SeqCst);

        let waker_ptr = self.waker.swap(null_mut(), Ordering::AcqRel);
        if !waker_ptr.is_null() {
            unsafe { Box::from_raw(waker_ptr).wake() }
        }
    }

    #[inline]
    fn store_waker(&self, waker: &Waker) {
        self.waker
            .store(Box::into_raw(Box::new(waker.clone())), Ordering::Release);
    }
}

/// The Simple Mutex Guard
/// As long as you have this guard, you have exclusive access to the underlying `T`. The guard internally borrows the Mutex, so the mutex will not be dropped while a guard exists.
/// The lock is automatically released and waked the next locker whenever the guard is dropped, at which point lock will succeed yet again.
pub struct UnorderedMutexGuard<'a, T: ?Sized> {
    mutex: &'a UnorderedMutex<T>,
}

pub struct UnorderedMutexGuardFuture<'a, T: ?Sized> {
    mutex: &'a UnorderedMutex<T>,
    is_realized: bool,
}

/// An owned handle to a held Mutex.
/// This guard is only available from a Mutex that is wrapped in an `Arc`. It is identical to `UnorderedMutexGuard`, except that rather than borrowing the `Mutex`, it clones the `Arc`, incrementing the reference count. This means that unlike `UnorderedMutexGuard`, it will have the `'static` lifetime.
/// As long as you have this guard, you have exclusive access to the underlying `T`. The guard internally keeps a reference-couned pointer to the original `Mutex`, so even if the lock goes away, the guard remains valid.
/// The lock is automatically released and waked the next locker whenever the guard is dropped, at which point lock will succeed yet again.
pub struct UnorderedMutexOwnedGuard<T: ?Sized> {
    mutex: Arc<UnorderedMutex<T>>,
}

pub struct UnorderedMutexOwnedGuardFuture<T: ?Sized> {
    mutex: Arc<UnorderedMutex<T>>,
    is_realized: bool,
}

unsafe impl<T> Send for UnorderedMutex<T> where T: ?Sized + Send {}
unsafe impl<T> Sync for UnorderedMutex<T> where T: ?Sized + Send {}

unsafe impl<T> Send for UnorderedMutexGuard<'_, T> where T: ?Sized + Send {}
unsafe impl<T> Send for UnorderedMutexOwnedGuard<T> where T: ?Sized + Send {}

unsafe impl<T> Sync for UnorderedMutexGuard<'_, T> where T: ?Sized + Send + Sync {}
unsafe impl<T> Sync for UnorderedMutexOwnedGuard<T> where T: ?Sized + Send + Sync {}

impl<'a, T: ?Sized> Future for UnorderedMutexGuardFuture<'a, T> {
    type Output = UnorderedMutexGuard<'a, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.mutex.is_acquired.swap(true, Ordering::AcqRel) {
            self.is_realized = true;
            Poll::Ready(UnorderedMutexGuard { mutex: self.mutex })
        } else {
            self.mutex.store_waker(cx.waker());
            Poll::Pending
        }
    }
}

impl<T: ?Sized> Future for UnorderedMutexOwnedGuardFuture<T> {
    type Output = UnorderedMutexOwnedGuard<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.mutex.is_acquired.swap(true, Ordering::AcqRel) {
            self.is_realized = true;
            Poll::Ready(UnorderedMutexOwnedGuard {
                mutex: self.mutex.clone(),
            })
        } else {
            self.mutex.store_waker(cx.waker());
            Poll::Pending
        }
    }
}

impl<T: ?Sized> Deref for UnorderedMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for UnorderedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Deref for UnorderedMutexOwnedGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for UnorderedMutexOwnedGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for UnorderedMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock()
    }
}

impl<T: ?Sized> Drop for UnorderedMutexOwnedGuard<T> {
    fn drop(&mut self) {
        self.mutex.unlock()
    }
}

impl<T: ?Sized> Drop for UnorderedMutexGuardFuture<'_, T> {
    fn drop(&mut self) {
        if !self.is_realized {
            self.mutex.unlock()
        }
    }
}

impl<T: ?Sized> Drop for UnorderedMutexOwnedGuardFuture<T> {
    fn drop(&mut self) {
        if !self.is_realized {
            self.mutex.unlock()
        }
    }
}

impl<T: Debug> Debug for UnorderedMutex<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnorderedMutex")
            .field("is_acquired", &self.is_acquired)
            .field("waker", &self.waker)
            .field("data", &self.data)
            .finish()
    }
}

impl<T: Debug> Debug for UnorderedMutexGuardFuture<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnorderedMutexGuardFuture")
            .field("mutex", &self.mutex)
            .field("is_realized", &self.is_realized)
            .finish()
    }
}

impl<T: Debug> Debug for UnorderedMutexGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnorderedMutexGuard")
            .field("mutex", &self.mutex)
            .finish()
    }
}

impl<T: Debug> Debug for UnorderedMutexOwnedGuardFuture<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnorderedMutexOwnedGuardFuture")
            .field("mutex", &self.mutex)
            .field("is_realized", &self.is_realized)
            .finish()
    }
}

impl<T: Debug> Debug for UnorderedMutexOwnedGuard<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnorderedMutexOwnedGuard")
            .field("mutex", &self.mutex)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::mutex_unordered::{UnorderedMutex, UnorderedMutexGuard, UnorderedMutexOwnedGuard};
    use futures::executor::block_on;
    use futures::{FutureExt, StreamExt, TryStreamExt};
    use std::ops::AddAssign;
    use std::sync::Arc;
    use tokio::time::{delay_for, Duration};

    #[tokio::test(core_threads = 12)]
    async fn test_mutex() {
        let c = UnorderedMutex::new(0);

        futures::stream::iter(0..10000)
            .for_each_concurrent(None, |_| async {
                let mut co: UnorderedMutexGuard<i32> = c.lock().await;
                *co += 1;
            })
            .await;

        let co = c.lock().await;
        assert_eq!(*co, 10000)
    }

    #[tokio::test(core_threads = 12)]
    async fn test_mutex_delay() {
        let expected_result = 100;
        let c = UnorderedMutex::new(0);

        futures::stream::iter(0..expected_result)
            .then(|i| c.lock().map(move |co| (i, co)))
            .for_each_concurrent(None, |(i, mut co)| async move {
                delay_for(Duration::from_millis(expected_result - i)).await;
                *co += 1;
            })
            .await;

        let co = c.lock().await;
        assert_eq!(*co, expected_result)
    }

    #[tokio::test(core_threads = 12)]
    async fn test_owned_mutex() {
        let c = Arc::new(UnorderedMutex::new(0));

        futures::stream::iter(0..10000)
            .for_each_concurrent(None, |_| async {
                let mut co: UnorderedMutexOwnedGuard<i32> = c.lock_owned().await;
                *co += 1;
            })
            .await;

        let co = c.lock_owned().await;
        assert_eq!(*co, 10000)
    }

    #[tokio::test]
    async fn test_container() {
        let c = UnorderedMutex::new(String::from("lol"));

        let mut co: UnorderedMutexGuard<String> = c.lock().await;
        co.add_assign("lol");

        assert_eq!(*co, "lollol");
    }

    #[tokio::test]
    async fn test_timeout() {
        let c = UnorderedMutex::new(String::from("lol"));

        let co: UnorderedMutexGuard<String> = c.lock().await;

        futures::stream::iter(0..10000i32)
            .then(|_| tokio::time::timeout(Duration::from_nanos(1), c.lock()))
            .try_for_each_concurrent(None, |_c| futures::future::ok(()))
            .await
            .expect_err("timout must be");

        drop(co);

        let mut co: UnorderedMutexGuard<String> = c.lock().await;
        co.add_assign("lol");

        assert_eq!(*co, "lollol");
    }

    #[test]
    fn multithreading_test() {
        let num = 100;
        let mutex = Arc::new(UnorderedMutex::new(0));
        let ths: Vec<_> = (0..num)
            .map(|_| {
                let mutex = mutex.clone();
                std::thread::spawn(move || {
                    block_on(async {
                        let mut lock = mutex.lock().await;
                        *lock += 1;
                    })
                })
            })
            .collect();

        for thread in ths {
            thread.join().unwrap();
        }

        block_on(async {
            let lock = mutex.lock().await;
            assert_eq!(num, *lock)
        })
    }
}
