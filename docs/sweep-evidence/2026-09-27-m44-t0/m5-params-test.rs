// M44 t0 M5(iii): the throwaway test that produced m5-params.log. It ran as
// crates/retrace/tests/t0_m5.rs (deleted after the run, never committed there) with
// `export T0_TRACE=<$L/threadrust.bin>` then
// `cargo test -p retrace --test t0_m5 -- --test-threads=1 --nocapture`. It computes the lldb
// sessions' parameters the way lldb_e2e does (threadrust_block's search, trap_pc,
// continue_to_window's ignore count, dbg_regs_of's pc), over one CLI recording of threadrust.
mod util;
use std::path::Path;
use retrace_trace::Event;

#[test]
fn t0_m5_params() {
    let tr_s = std::env::var("T0_TRACE").expect("T0_TRACE");
    let tr = Path::new(&tr_s);
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let thread_of = |e: &Event| match e { Event::Syscall { thread, .. } => Some(*thread), _ => None };
    let num_of = |e: &Event| match e { Event::Syscall { num, .. } => Some(*num as i64), _ => None };
    println!("T0M5 events={}", ev.len());
    let n = (1..ev.len() - 1).find(|&i| matches!(ev[i], Event::Syscall { num: 515, .. })
        && thread_of(&ev[i + 1]).is_some() && thread_of(&ev[i + 1]) != thread_of(&ev[i])).unwrap();
    let t = thread_of(&ev[n]).unwrap();
    let svc = util::rsp::trap_pc(tr, n);
    let b = retrace_core::seek(tr, n + 1, 0).unwrap().pc();
    let m = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, n).0
    };
    let other = if t == 0 { 2u64 } else { 1 };
    let other_pc = {
        let len = retrace_core::seek(tr, n, 0).unwrap().window_len_here().unwrap();
        let s = retrace_core::seek(tr, n, len).unwrap();
        util::rsp::dbg_field(&s.dbg_regs_of(other as usize - 1).unwrap(), "pc")
    };
    println!("T0M5 A/B n={n} t={t} me={} svc={svc:#x} m={m} b={b:#x} other={other} other_pc={other_pc:#x}", t + 1);
    // Every thread's landmarks: count, first, last (num).
    for th in 0..8u32 {
        let idx: Vec<usize> = (0..ev.len()).filter(|&i| thread_of(&ev[i]) == Some(th)).collect();
        if idx.is_empty() { continue; }
        let last = *idx.last().unwrap();
        println!("T0M5 thread {th}: landmarks={} first={} last={} last_num={:?} tail={:?}", idx.len(), idx[0], last,
            num_of(&ev[last]), idx.iter().rev().take(5).map(|&i| (i, num_of(&ev[i]))).collect::<Vec<_>>());
    }
    // Shape C: the child's (thread 1's) last landmark, its svc, and the ignore count to reach it.
    let lc = (0..ev.len()).rev().find(|&i| thread_of(&ev[i]) == Some(1)).unwrap();
    let svc_c = util::rsp::trap_pc(tr, lc);
    let m_c = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc_c:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, lc).0
    };
    println!("T0M5 C lc={lc} num={:?} svc_c={svc_c:#x} m_c={m_c} next={:?}", num_of(&ev[lc]),
        ev.get(lc + 1).map(|e| (thread_of(e), num_of(e))));
    for (i, e) in ev.iter().enumerate().skip(lc.saturating_sub(3)).take(8) {
        println!("T0M5 ev[{i}] thread={:?} num={:?}", thread_of(e), num_of(e));
    }
    println!("T0M5 last={:?}", ev.last().map(|e| std::mem::discriminant(e)));
}
