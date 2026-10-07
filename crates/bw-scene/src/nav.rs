//! Keyboard navigation of the process tree (PLAN §10): a logical focus model
//! independent of 3D picking, so every entity is reachable without a mouse.
//!
//! ↑ parent · ↓ child · ←/→ siblings · Tab next busiest · Esc clear.

use crate::camera::InputBlock;
use crate::*;
use bw_model::Realm;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        navigate.after(SceneSet::Ingest).before(SceneSet::Layout),
    );
}

/// Siblings of `k` in display order (same parent, or same kernel subsystem).
pub fn siblings(m: &Machine, layout: &Layout, k: ProcKey) -> Vec<ProcKey> {
    let s = &m.snapshot;
    let Some(p) = s.processes.get(&k) else {
        return vec![];
    };
    let mut v: Vec<ProcKey> = match p.realm {
        Realm::Kernel => {
            let sub = layout.subsystem_of(&k);
            s.processes
                .values()
                .filter(|q| q.realm == Realm::Kernel && layout.subsystem_of(&q.key) == sub)
                .map(|q| q.key)
                .collect()
        }
        Realm::User => match p
            .parent
            .filter(|pp| s.processes.get(pp).is_some_and(|q| q.realm == Realm::User))
        {
            Some(pp) => s
                .processes
                .values()
                .filter(|q| q.parent == Some(pp))
                .map(|q| q.key)
                .collect(),
            None => s
                .roots()
                .into_iter()
                .filter(|r| s.processes[r].realm == Realm::User)
                .collect(),
        },
    };
    v.sort();
    v
}

/// Processes sorted by CPU, busiest first.
pub fn busiest(m: &Machine) -> Vec<ProcKey> {
    let mut v: Vec<_> = m.snapshot.processes.values().collect();
    v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct).then(a.key.cmp(&b.key)));
    v.into_iter().map(|p| p.key).collect()
}

fn navigate(
    keys: Res<ButtonInput<KeyCode>>,
    block: Res<InputBlock>,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    mut sel: ResMut<Selection>,
    ex: Res<crate::explore::Explore>,
) {
    // Inside a process the arrows walk its interior instead (explore.rs).
    if ex.inside().is_some()
        || block.keyboard
        || keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
    {
        return;
    }
    let s = &m.snapshot;
    if keys.just_pressed(KeyCode::Escape) {
        sel.key = None;
        return;
    }
    if keys.just_pressed(KeyCode::Tab) {
        let top: Vec<_> = busiest(&m).into_iter().take(12).collect();
        let next = match sel.key.and_then(|k| top.iter().position(|t| *t == k)) {
            Some(i) => top.get((i + 1) % top.len()).copied(),
            None => top.first().copied(),
        };
        if next.is_some() {
            sel.key = next;
            sel.follow = true;
        }
        return;
    }
    let Some(cur) = sel.key.filter(|k| s.processes.contains_key(k)) else {
        // Any arrow with nothing selected starts at the root of Deep Space.
        if [
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ]
        .iter()
        .any(|k| keys.just_pressed(*k))
        {
            sel.key = s
                .processes
                .get(&ProcKey {
                    pid: 1,
                    start_time: 0,
                })
                .map(|p| p.key)
                .or_else(|| {
                    s.roots()
                        .into_iter()
                        .filter(|r| s.processes[r].realm == Realm::User)
                        .max_by_key(|r| s.children().get(r).map_or(0, Vec::len))
                });
            sel.follow = true;
        }
        return;
    };
    let mut next = None;
    if keys.just_pressed(KeyCode::ArrowUp) {
        next = s.processes[&cur]
            .parent
            .filter(|p| s.processes.contains_key(p));
        if let Some(p) = next {
            sel.last_child.insert(p, cur);
        }
    } else if keys.just_pressed(KeyCode::ArrowDown) {
        let kids = s.children();
        let mut kids: Vec<ProcKey> = kids.get(&cur).cloned().unwrap_or_default();
        kids.sort();
        next = sel
            .last_child
            .get(&cur)
            .copied()
            .filter(|c| kids.contains(c))
            .or(kids.first().copied());
    } else if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::ArrowRight) {
        let sib = siblings(&m, &sl.layout, cur);
        if let Some(i) = sib.iter().position(|k| *k == cur) {
            let n = sib.len();
            let j = if keys.just_pressed(KeyCode::ArrowRight) {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            next = Some(sib[j]);
        }
    }
    if let Some(n) = next {
        sel.key = Some(n);
        sel.follow = true;
    }
}
