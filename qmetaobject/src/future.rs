use crate::connections::SignalArgArrayToTuple;
use std::future::Future;
use std::os::raw::c_void;
use std::pin::Pin;

static QTWAKERVTABLE: std::task::RawWakerVTable = unsafe {
    std::task::RawWakerVTable::new(
        |s: *const ()| {
            std::task::RawWaker::new(
                cpp!([s as "Waker*"] -> *const() as "Waker*" {
                    s->ref++;
                    return s;
                }),
                &QTWAKERVTABLE,
            )
        },
        |s: *const ()| {
            cpp!([s as "Waker*"] {
                s->wake();
                s->deref();
            })
        },
        |s: *const ()| {
            cpp!([s as "Waker*"] {
                s->wake();
            })
        },
        |s: *const ()| {
            cpp!([s as "Waker*"] {
                s->deref();
            })
        },
    )
};

cpp! {{
    struct Waker : QObject {
    public:
        TraitObject future;
        bool woken = false;
        bool completed = false;
        QAtomicInt ref = 0;
        void customEvent(QEvent *e) override {
            Q_UNUSED(e);
            woken = false;
            // future must not be polled after it returned `Poll::Ready`
            if (completed) return;
            completed = rust!(ProcessQtEvent [this: *const() as "Waker*",
                future : *mut dyn Future<Output=()> as "TraitObject"] -> bool as "bool" {
                poll_with_qt_waker(this, Pin::new_unchecked(&mut *future))
            });
            if (completed) deref();
        }
        void deref() {
            if (!--ref) {
                deleteLater();
            }
        }
        void wake() {
            if (woken) return;
            woken = true;
            QApplication::postEvent(this, new QEvent(QEvent::User));
        }
        ~Waker() {
            rust!(QtDestroyFuture [future : *mut dyn Future<Output=()> as "TraitObject"] {
                std::mem::drop(Box::from_raw(future))
            });
        }
    };
}}

/// Execute a future on the Qt Event loop
///
/// Waking the waker will post an event to the Qt event loop which will poll the future
/// from the event handler
///
/// Note that this function returns immediatly. A Qt event loop need to be running
/// on the current thread so the future can be executed. (It is Ok if the Qt event
/// loop hasn't started yet when this function is called)
pub fn execute_async(f: impl Future<Output = ()> + 'static) {
    let f = Box::into_raw(Box::new(f)) as *mut dyn Future<Output = ()>;
    unsafe {
        let waker = cpp!([f as "TraitObject"] -> *const() as "Waker*" {
            auto w = new Waker;
            w->ref++;
            w->future = f;
            return w;
        });
        poll_with_qt_waker(waker, Pin::new_unchecked(&mut *f));
    }
}

unsafe fn poll_with_qt_waker(waker: *const (), future: Pin<&mut dyn Future<Output = ()>>) -> bool {
    cpp!([waker as "Waker*"] { waker->ref++; });
    let waker = std::task::RawWaker::new(waker, &QTWAKERVTABLE);
    let waker = std::task::Waker::from_raw(waker);
    let mut context = std::task::Context::from_waker(&waker);
    future.poll(&mut context).is_ready()
}

/// Create a future that waits on the emission of a signal.
///
/// The arguments of the signal need to implement `Clone`, and the Output of the future is a tuple
/// containing the arguments of the signal (or the empty tuple if there are none.)
///
/// The future will be ready as soon as the signal is emited.
///
/// This is unsafe for the same reason that connections::connect is unsafe.
pub unsafe fn wait_on_signal<Args: SignalArgArrayToTuple>(
    sender: *const c_void,
    signal: crate::connections::CppSignal<Args>,
) -> impl Future<Output = <Args as SignalArgArrayToTuple>::Tuple> {
    enum ConnectionFutureState<Args: SignalArgArrayToTuple> {
        Init {
            sender: *const c_void,
            signal: crate::connections::CppSignal<Args>,
        },
        Started {
            handle: crate::connections::ConnectionHandle,
            waker: std::task::Waker,
        },
        Finished {
            result: <Args as SignalArgArrayToTuple>::Tuple,
        },
        Invalid,
    }
    impl<Args: SignalArgArrayToTuple> std::marker::Unpin for ConnectionFutureState<Args> {}
    struct ConnectionFuture<Args: SignalArgArrayToTuple>(ConnectionFutureState<Args>);
    impl<Args: SignalArgArrayToTuple> Drop for ConnectionFuture<Args> {
        fn drop(&mut self) {
            if let ConnectionFutureState::Started { ref mut handle, .. } = &mut self.0 {
                handle.disconnect();
            }
        }
    }
    impl<Args: SignalArgArrayToTuple> Future for ConnectionFuture<Args> {
        type Output = <Args as SignalArgArrayToTuple>::Tuple;
        fn poll(
            mut self: Pin<&mut Self>,
            ctx: &mut std::task::Context,
        ) -> std::task::Poll<Self::Output> {
            let state = &mut self.0;
            *state = match std::mem::replace(state, ConnectionFutureState::Invalid) {
                ConnectionFutureState::Finished { result } => {
                    return std::task::Poll::Ready(result);
                }
                ConnectionFutureState::Init { sender, signal } => {
                    let s_ptr = state as *mut ConnectionFutureState<_>;
                    let handle = unsafe { crate::connections::connect(sender, signal, s_ptr) };
                    debug_assert!(handle.is_valid());
                    ConnectionFutureState::Started {
                        handle,
                        waker: ctx.waker().clone(),
                    }
                }
                s @ ConnectionFutureState::Started { .. } => s,
                ConnectionFutureState::Invalid => unreachable!(),
            };
            std::task::Poll::Pending
        }
    }

    impl<Args: SignalArgArrayToTuple> crate::connections::Slot<Args>
        for *mut ConnectionFutureState<Args>
    {
        unsafe fn apply(&mut self, a: *const *const c_void) {
            if let ConnectionFutureState::Started { mut handle, waker } = std::mem::replace(
                &mut **self,
                ConnectionFutureState::Finished {
                    result: Args::args_array_to_tuple(a),
                },
            ) {
                handle.disconnect();
                waker.wake();
            } else {
                unreachable!();
            }
        }
    }

    ConnectionFuture(ConnectionFutureState::Init { sender, signal })
}
