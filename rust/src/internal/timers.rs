// Waiting, for cpeak::sleep(). In Node.js this is setTimeout(), which comes with the runtime.
//
// tokio has timers of its own, but they work in whole milliseconds, and round up twice: the
// time to wait, and the time the event loop sleeps until the next timer is due. A wait of
// 10ms takes 11 to 12ms that way. A server that waits on its requests (for a database, for
// another service, ...) can only answer as many of them as those waits allow, so that
// millisecond is a tenth of its throughput.
//
// So on Linux, each worker thread keeps its own timers, and asks the operating system for a
// wake-up call at the moment the next one is due (a timerfd, which its event loop watches
// like a socket). Elsewhere, and outside of Cpeak's worker threads, we wait with tokio.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

// Wait for the given time without holding up the thread, like
// `await new Promise((resolve) => setTimeout(resolve, ms))` in Node.js:
//
//     cpeak::sleep(Duration::from_millis(100)).await;
pub async fn sleep(duration: Duration) {
    Sleep::new(Instant::now() + duration).await
}

struct Sleep {
    deadline: Instant,
    state: State,
}

enum State {
    // Not polled yet
    New,
    // One of this worker thread's own timers (Linux)
    #[cfg(target_os = "linux")]
    Waiting(linux::Timer),
    // tokio's timer
    Tokio(Pin<Box<tokio::time::Sleep>>),
}

impl Sleep {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            state: State::New,
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        loop {
            match &mut this.state {
                State::New => {
                    if Instant::now() >= this.deadline {
                        return Poll::Ready(());
                    }
                    #[cfg(target_os = "linux")]
                    if let Some(timer) = linux::Timer::start(this.deadline, cx) {
                        this.state = State::Waiting(timer);
                        return Poll::Pending;
                    }
                    this.state =
                        State::Tokio(Box::pin(tokio::time::sleep_until(this.deadline.into())));
                }
                #[cfg(target_os = "linux")]
                State::Waiting(timer) => return timer.poll(this.deadline, cx),
                State::Tokio(sleep) => return sleep.as_mut().poll(cx),
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use self::linux::start;

// Elsewhere there is nothing to set up: we wait with tokio
#[cfg(not(target_os = "linux"))]
pub(crate) fn start(_runtime: &tokio::runtime::Runtime) {}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use tokio::io::unix::AsyncFd;
    use tokio::io::Interest;

    // Timers that are due within the same stretch of this length are woken up together, so a
    // worker with thousands of them isn't woken up for each one. A timer can be this late.
    const GRANULARITY: Duration = Duration::from_micros(100);

    thread_local! {
        // The timers of this worker thread. None on every other thread.
        static TIMERS: RefCell<Option<Timers>> = const { RefCell::new(None) };
    }

    struct Timers {
        // The timerfd that wakes this thread's event loop up
        fd: RawFd,
        // What the wake-up times are counted from
        started: Instant,
        // When the timerfd is set to go off (None when it isn't set)
        wakes_at: Option<Instant>,
        // The timers that are waiting, the one that is due first on top
        queue: BinaryHeap<Reverse<(Instant, usize, u32)>>,
        slots: Vec<Slot>,
        free_slots: Vec<usize>,
    }

    // One timer. A slot is used again once its timer is done, so each use has its own number.
    struct Slot {
        used: u32,
        waker: Option<Waker>,
        fired: bool,
    }

    // A timer that is waiting, as cpeak::sleep() holds it
    pub(super) struct Timer {
        slot: usize,
        used: u32,
        // The thread it belongs to, as the address of that thread's TIMERS
        thread: usize,
    }

    fn this_thread() -> usize {
        TIMERS.with(|timers| timers as *const _ as usize)
    }

    impl Timer {
        // None when this thread isn't one of Cpeak's workers
        pub(super) fn start(deadline: Instant, cx: &Context<'_>) -> Option<Self> {
            TIMERS.with(|timers| {
                let mut timers = timers.borrow_mut();
                let timers = timers.as_mut()?;

                let slot = match timers.free_slots.pop() {
                    Some(slot) => slot,
                    None => {
                        timers.slots.push(Slot {
                            used: 0,
                            waker: None,
                            fired: false,
                        });
                        timers.slots.len() - 1
                    }
                };
                let used = timers.slots[slot].used;
                timers.slots[slot].waker = Some(cx.waker().clone());
                timers.slots[slot].fired = false;
                timers.queue.push(Reverse((deadline, slot, used)));

                // Due before everything we were waiting for: the wake-up call moves forward
                let wake_at = timers.wake_up_time(deadline);
                if timers.wakes_at.is_none_or(|wakes_at| wake_at < wakes_at) {
                    timers.wake_up_at(wake_at);
                }

                Some(Self {
                    slot,
                    used,
                    thread: this_thread(),
                })
            })
        }

        pub(super) fn poll(&mut self, deadline: Instant, cx: &Context<'_>) -> Poll<()> {
            // A timer never leaves its worker thread in Cpeak. If it does anyway, all we can
            // go by is the clock.
            if self.thread != this_thread() {
                return if Instant::now() >= deadline {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                };
            }

            TIMERS.with(|timers| {
                let mut timers = timers.borrow_mut();
                let Some(timers) = timers.as_mut() else {
                    return Poll::Ready(());
                };
                let slot = &mut timers.slots[self.slot];
                if slot.fired || Instant::now() >= deadline {
                    return Poll::Ready(());
                }
                match &mut slot.waker {
                    Some(waker) if waker.will_wake(cx.waker()) => {}
                    waker => *waker = Some(cx.waker().clone()),
                }
                Poll::Pending
            })
        }
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            if self.thread != this_thread() {
                return;
            }
            // Thread-locals can be gone already while a thread is shutting down
            let _ = TIMERS.try_with(|timers| {
                let Ok(mut timers) = timers.try_borrow_mut() else {
                    return;
                };
                let Some(timers) = timers.as_mut() else {
                    return;
                };
                // Give the slot back. If the timer is still in the queue (it was dropped
                // before it was due), the queue finds out by the slot's number having moved on.
                let slot = &mut timers.slots[self.slot];
                if slot.used == self.used {
                    slot.used = slot.used.wrapping_add(1);
                    slot.waker = None;
                    slot.fired = false;
                    timers.free_slots.push(self.slot);
                }
            });
        }
    }

    impl Timers {
        // The moment the event loop gets woken up for a timer that is due at `deadline`:
        // the next multiple of GRANULARITY
        fn wake_up_time(&self, deadline: Instant) -> Instant {
            let granularity = GRANULARITY.as_nanos() as u64;
            let since_start = deadline.saturating_duration_since(self.started).as_nanos() as u64;
            self.started + Duration::from_nanos(since_start.div_ceil(granularity) * granularity)
        }

        fn wake_up_at(&mut self, wake_at: Instant) {
            // A time of zero would switch the timerfd off, so it's at least a nanosecond from now
            let from_now = wake_at
                .saturating_duration_since(Instant::now())
                .max(Duration::from_nanos(1));
            let time = libc::itimerspec {
                it_interval: libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                it_value: libc::timespec {
                    tv_sec: from_now.as_secs() as libc::time_t,
                    tv_nsec: from_now.subsec_nanos() as libc::c_long,
                },
            };
            // SAFETY: fd is our timerfd, and `time` is a valid itimerspec
            unsafe { libc::timerfd_settime(self.fd, 0, &time, std::ptr::null_mut()) };
            self.wakes_at = Some(wake_at);
        }

        // Wake up everyone whose time has come, and set the wake-up call for the next one
        fn fire(&mut self) -> Vec<Waker> {
            let now = Instant::now();
            let mut wakers = Vec::new();
            self.wakes_at = None;

            while let Some(&Reverse((deadline, slot, used))) = self.queue.peek() {
                let slot = &mut self.slots[slot];
                if slot.used != used {
                    // This one was dropped before it was due
                    self.queue.pop();
                    continue;
                }
                if deadline > now {
                    let wake_at = self.wake_up_time(deadline);
                    self.wake_up_at(wake_at);
                    break;
                }
                self.queue.pop();
                slot.fired = true;
                if let Some(waker) = slot.waker.take() {
                    wakers.push(waker);
                }
            }
            wakers
        }
    }

    // Give this worker thread its timers. Called once, from the worker's own thread.
    pub(crate) fn start(runtime: &tokio::runtime::Runtime) {
        // SAFETY: plain system call, and we own the file descriptor it gives back
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        };
        if fd < 0 {
            // No timerfd for us: cpeak::sleep() waits with tokio on this thread
            return;
        }
        // SAFETY: fd was just created, and nothing else owns it
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let _in_runtime = runtime.enter();
        let Ok(timer) = AsyncFd::with_interest(fd, Interest::READABLE) else {
            return;
        };

        TIMERS.with(|timers| {
            *timers.borrow_mut() = Some(Timers {
                fd: timer.get_ref().as_raw_fd(),
                started: Instant::now(),
                wakes_at: None,
                queue: BinaryHeap::new(),
                slots: Vec::new(),
                free_slots: Vec::new(),
            });
        });

        // Runs on this thread for as long as the worker does: every time the timerfd goes
        // off, it wakes up the requests whose wait is over
        runtime.spawn(async move {
            loop {
                let Ok(mut went_off) = timer.readable().await else {
                    break;
                };
                // Setting the timerfd again is what resets it, so there's nothing to read
                went_off.clear_ready();

                let wakers = TIMERS.with(|timers| match timers.borrow_mut().as_mut() {
                    Some(timers) => timers.fire(),
                    None => Vec::new(),
                });
                for waker in wakers {
                    waker.wake();
                }
            }
        });
    }
}
