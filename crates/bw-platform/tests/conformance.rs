//! Platform conformance suite: the same checks run on every OS in CI (PLAN §3.9).

use bw_platform::{Collector, SysCollector};

#[test]
fn sees_this_process_and_sane_values() {
    let mut c = SysCollector::new();
    std::thread::sleep(std::time::Duration::from_millis(250));
    let s = c.sample();

    let me = std::process::id();
    let mine = s
        .processes
        .values()
        .find(|p| p.key.pid == me)
        .expect("own process is visible");
    assert_eq!(mine.realm, bw_model::Realm::User);
    assert!(mine.mem_bytes > 0, "own RSS should be non-zero");

    // Parent chain terminates (no cycles) within a sane depth.
    let mut cur = mine.parent;
    let mut depth = 0;
    while let Some(k) = cur {
        depth += 1;
        assert!(depth < 256, "parent chain too deep: cycle?");
        cur = s.processes.get(&k).and_then(|p| p.parent);
    }

    assert!(s.system.mem_total > 0);
    assert!(s.system.mem_used <= s.system.mem_total);
    assert!((0.0..=1.0).contains(&s.system.kernel_pressure));
    assert!(s.host.cpu_count > 0);
    for p in s.processes.values() {
        assert!(p.cpu_pct >= 0.0 && p.cpu_pct.is_finite());
    }
}
