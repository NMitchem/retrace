// M52 first-contact fixture (spec §3j): a `rustc --test` harness whose tests do what a newcomer's own
// suite does with threads: contend a mutex, wait on a condvar, time out on a channel, park, yield
// and read thread ids. libtest runs them on its own pool, so the result lines come in a varying
// order natively; firstcontact_e2e compares the output canonically. Each test is deterministic.
#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::Duration;

    extern "C" {
        fn pthread_threadid_np(t: usize, id: *mut u64) -> i32;
    }

    #[test]
    fn a_contended_mutex_counts_exactly() {
        let m = Arc::new(Mutex::new(0u32));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let m = m.clone();
                thread::spawn(move || {
                    for _ in 0..1000 {
                        *m.lock().unwrap() += 1;
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*m.lock().unwrap(), 4000);
    }

    #[test]
    fn a_condvar_signalled_under_its_contended_mutex() {
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let waiter = {
            let p = pair.clone();
            thread::spawn(move || {
                let (m, cv) = &*p;
                let mut g = m.lock().unwrap();
                while !*g {
                    g = cv.wait(g).unwrap();
                }
            })
        };
        let signaller = {
            let p = pair.clone();
            thread::spawn(move || {
                let (m, cv) = &*p;
                let mut g = m.lock().unwrap();
                *g = true;
                cv.notify_all();
                // Hold the mutex a while so the third thread contends.
                for _ in 0..1000 {
                    std::hint::black_box(&mut *g);
                }
            })
        };
        let contender = {
            let p = pair.clone();
            thread::spawn(move || {
                let (m, _) = &*p;
                for _ in 0..100 {
                    drop(m.lock().unwrap());
                }
            })
        };
        waiter.join().unwrap();
        signaller.join().unwrap();
        contender.join().unwrap();
    }

    #[test]
    fn recv_timeout_expires_and_is_satisfied() {
        let (tx, rx) = channel::<u32>();
        assert_eq!(rx.recv_timeout(Duration::from_millis(20)), Err(RecvTimeoutError::Timeout));
        let h = thread::spawn(move || tx.send(7).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(7));
        h.join().unwrap();
    }

    #[test]
    fn park_timeout_with_and_without_unpark() {
        thread::park_timeout(Duration::from_millis(20));
        let (tx, rx) = channel::<()>();
        let h = thread::spawn(move || {
            tx.send(()).unwrap();
            thread::park_timeout(Duration::from_secs(5));
        });
        rx.recv().unwrap();
        h.thread().unpark();
        h.join().unwrap();
    }

    #[test]
    fn a_yield_loop_sees_a_flag_another_thread_sets() {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let h = thread::spawn(move || f.store(true, Ordering::SeqCst));
        while !flag.load(Ordering::SeqCst) {
            thread::yield_now();
        }
        h.join().unwrap();
    }

    fn tid() -> u64 {
        let mut id = 0u64;
        assert_eq!(unsafe { pthread_threadid_np(0, &mut id) }, 0);
        id
    }

    #[test]
    fn every_thread_has_a_distinct_id() {
        let hs: Vec<_> = (0..3).map(|_| thread::spawn(|| (tid(), thread::sleep(Duration::from_millis(5))).0)).collect();
        let mut ids = vec![tid()];
        for h in hs {
            ids.push(h.join().unwrap());
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn a_poisoned_mutex_is_still_lockable() {
        let m = Arc::new(Mutex::new(1u32));
        let p = m.clone();
        assert!(thread::spawn(move || {
            let _g = p.lock().unwrap();
            panic!("poisoning on purpose");
        })
        .join()
        .is_err());
        let hs: Vec<_> = (0..2)
            .map(|_| {
                let m = m.clone();
                thread::spawn(move || *m.lock().unwrap_or_else(|e| e.into_inner()))
            })
            .collect();
        for h in hs {
            assert_eq!(h.join().unwrap(), 1);
        }
    }
}
