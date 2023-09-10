use core::task::Waker;
use futures::{future::BoxFuture};
use scoped_tls::scoped_thread_local;
use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex},
    task::{Context, Poll, RawWaker, RawWakerVTable, Wake},
};

struct Demo;

impl Future for Demo {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        println!("hello world!");
        std::task::Poll::Ready(())
    }
}

fn dummy_waker() -> Waker {
    static DATA: () = ();
    unsafe { Waker::from_raw(RawWaker::new(&DATA, &VTABLE)) }
}

const VTABLE: RawWakerVTable =
    RawWakerVTable::new(vtable_clone, vtable_wake, vtable_wake_by_ref, vtable_drop);

unsafe fn vtable_clone(_p: *const ()) -> RawWaker {
    RawWaker::new(_p, &VTABLE)
}

unsafe fn vtable_wake(_p: *const ()) {}
unsafe fn vtable_wake_by_ref(_p: *const ()) {}
unsafe fn vtable_drop(_p: *const ()) {}

struct Signal {
    state: Mutex<State>,
    cond: Condvar,
}

enum State {
    Empty,
    Waiting,
    Notified,
}
impl Signal {
    fn wait(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            State::Notified => *state = State::Empty,
            State::Waiting => {
                panic!("multiple waiting");
            }
            State::Empty => {
                *state = State::Waiting;
                while let State::Waiting = *state {
                    state = self.cond.wait(state).unwrap();
                }
            }
        }
    }

    fn notify(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            State::Notified => {}
            State::Empty => *state = State::Notified,
            State::Waiting => {
                *state = State::Empty;
                self.cond.notify_one();
            }
        }
    }
    fn new() -> Signal {
        Signal {
            state: Mutex::new(State::Empty),
            cond: Condvar::new(),
        }
    }
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
}

struct Task {
    future: RefCell<BoxFuture<'static, ()>>,
    signal: Arc<Signal>,
}

impl Task {
    fn new(future: impl Future<Output = ()> + 'static + Send, signal: Arc<Signal>) -> Task {
        Task {
            future: RefCell::new(Box::pin(future)),
            signal: signal,
        }
    }
}
unsafe impl Send for Task {}
unsafe impl Sync for Task {}

scoped_thread_local!(static SIGNAL: Arc<Signal>);
scoped_thread_local!(static RUNNABLE: Mutex<VecDeque<Arc<Task>>>);

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        RUNNABLE.with(|runnable| runnable.lock().unwrap().push_back(self.clone()));
        self.signal.notify();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut fut = std::pin::pin!(future);
    let signal = Arc::new(Signal::new());
    let waker = Waker::from(signal.clone());
    let mut cx = Context::from_waker(&waker);
    let runnable = Mutex::new(VecDeque::with_capacity(1024));
    SIGNAL.set(&signal, || {
        RUNNABLE.set(&runnable, || loop {
            if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
                return output;
            }
            while let Some(task) = runnable.lock().unwrap().pop_front() {
                let waker = Waker::from(task.clone());
                let mut cx = Context::from_waker(&waker);
                let _ = task.future.borrow_mut().as_mut().poll(&mut cx);
            }
            signal.wait();
        })
    })
}

fn myspawn(future: impl Future<Output = ()> + 'static + Send) {
    let signal = Arc::new(Signal::new());
    let waker = Waker::from(signal.clone());
    let task = Arc::new(Task::new(future, signal.clone()));
    let mut cx = Context::from_waker(&waker);
    if let Poll::Ready(_) = task.future.borrow_mut().as_mut().poll(&mut cx) {
        return;
    }
    RUNNABLE.with(|runnable| {
        runnable.lock().unwrap().push_back(task);
        signal.notify();
    })
}

async fn demo() {
    let (tx, rx) = async_channel::bounded::<()>(1);
    println!("hello world1 !");
    myspawn(demo2(tx));
    let _ = rx.recv().await;
}

async fn demo2(tx: async_channel::Sender<()>) {
    println!("hello world2 !");
    let _ = tx.send(()).await;
}

fn main() {
    block_on(demo());
}
