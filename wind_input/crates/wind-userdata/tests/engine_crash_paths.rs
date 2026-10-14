//! 引擎级崩溃 / 并发路径（设计 §11）。
use wind_userdata::engine::{Dirs, UserData};
use wind_userdata::record::{OpKind, Payload, Target};

fn dirs(name: &str) -> Dirs {
    let d = std::env::temp_dir().join(format!("wind_ud_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Dirs {
        state_dir: d,
        dir_id: "dir".into(),
    }
}

fn texts(ud: &UserData, code: &str) -> Vec<String> {
    ud.query_exact(Target::Temp, "py", code)
        .into_iter()
        .map(|(t, _w)| t)
        .collect()
}

#[test]
fn write_is_visible_immediately_and_survives_reopen() {
    let d = dirs("vis");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "nihao",
        "你好",
        Payload::Weight(10),
        0b101,
    )
    .unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "写后立即可读");
    drop(ud);
    let ud = UserData::open(d).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "重开后由日志回放可读");
}

#[test]
fn fold_then_reopen_reads_from_snapshot_without_journal_replay() {
    let d = dirs("fold");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "nihao",
        "你好",
        Payload::Weight(10),
        0b101,
    )
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "nihao",
        "拟好",
        Payload::Weight(3),
        0b101,
    )
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Delete,
        "nihao",
        "拟好",
        Payload::None,
        0b101,
    )
    .unwrap();
    ud.fold(Target::Temp, "py").unwrap();
    assert_eq!(
        texts(&ud, "nihao"),
        vec!["你好"],
        "折回后 delta 清空、快照里是叠加后的结果"
    );
    assert_eq!(ud.view().delta.len(), 0);
    drop(ud);
    let ud = UserData::open(d).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"]);
    assert_eq!(ud.view().delta.len(), 0, "已折回的记录不再回放");
}

/// Review Focus 2：state 缺失 ⇒ 以快照内嵌水位为准，只回放更大的 seq，不重复应用。
#[test]
fn state_missing_replays_only_beyond_snapshot_watermark() {
    let d = dirs("wm");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "a",
        "甲",
        Payload::Weight(1),
        0,
    )
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Incr,
        "a",
        "甲",
        Payload::Count(1),
        0,
    )
    .unwrap();
    ud.fold(Target::Temp, "py").unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Incr,
        "a",
        "甲",
        Payload::Count(1),
        0,
    )
    .unwrap(); // 折回后一条
    drop(ud);
    std::fs::remove_file(d.state_dir.join("journal/state")).unwrap();
    let ud = UserData::open(d).unwrap();
    assert_eq!(
        ud.query_count(Target::Temp, "py", "a", "甲"),
        2,
        "快照里 1 + 日志里只回放折回后的 1；重复回放会得 3"
    );
}

/// Review Focus 4：折回期间新到的操作保留。
#[test]
fn fold_keeps_ops_that_arrived_during_fold() {
    let d = dirs("race");
    let ud = std::sync::Arc::new(UserData::open(d).unwrap());
    for i in 0..500 {
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            &format!("c{i}"),
            "x",
            Payload::Weight(1),
            0,
        )
        .unwrap();
    }
    let w = {
        let ud = ud.clone();
        std::thread::spawn(move || {
            for i in 500..600 {
                ud.apply(
                    Target::Temp,
                    "py",
                    OpKind::Upsert,
                    &format!("c{i}"),
                    "x",
                    Payload::Weight(1),
                    0,
                )
                .unwrap();
            }
        })
    };
    ud.fold(Target::Temp, "py").unwrap();
    w.join().unwrap();
    for i in 0..600 {
        assert_eq!(
            ud.query_exact(Target::Temp, "py", &format!("c{i}")).len(),
            1,
            "c{i} 丢了"
        );
    }
}

#[test]
fn second_instance_is_readonly() {
    let d = dirs("ro");
    let a = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    let b = UserData::open(d).unwrap();
    assert!(!a.readonly() && b.readonly());
    assert!(
        b.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "a",
            "甲",
            Payload::Weight(1),
            0
        )
        .is_err()
    );
}

fn up(ud: &UserData, code: &str, text: &str) {
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        code,
        text,
        Payload::Weight(1),
        0,
    )
    .unwrap();
}

fn reopen(d: &Dirs) -> UserData {
    UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap()
}

/// 删掉 journal/（连同里面的 state）后，序号不能倒退到快照水位之下，否则新记录重开即丢。
#[test]
fn deleting_journal_dir_does_not_rewind_seq_below_snapshot() {
    let d = dirs("rmjnl");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    up(&ud, "b", "乙");
    ud.fold(Target::Temp, "py").unwrap();
    drop(ud);
    std::fs::remove_dir_all(d.state_dir.join("journal")).unwrap();
    let ud = reopen(&d);
    up(&ud, "c", "丙");
    up(&ud, "e", "戊");
    drop(ud);
    let ud = reopen(&d);
    assert_eq!(texts(&ud, "a"), vec!["甲"], "快照里的还在");
    assert_eq!(texts(&ud, "c"), vec!["丙"], "删目录后新写的重开可见");
    assert_eq!(texts(&ud, "e"), vec!["戊"]);
}

/// 快照真坏了（不是缺失）：不能信 state 的水位，要从日志整份回放该目标。
#[test]
fn corrupt_snapshot_falls_back_to_full_replay_of_that_target() {
    let d = dirs("corrupt");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    drop(ud);
    std::fs::write(d.state_dir.join("py").join("temp.meta"), b"WUDM").unwrap();
    let ud = reopen(&d);
    assert_eq!(texts(&ud, "a"), vec!["甲"]);
}

/// 只读实例（另一实例持锁）不得改动任何文件：日志尾巴可能是写者正在写的那一条。
#[test]
fn readonly_instance_never_writes_journal_or_device() {
    let d = dirs("rofiles");
    let w = reopen(&d);
    up(&w, "a", "甲");
    let jdir = d.state_dir.join("journal");
    let jpath = std::fs::read_dir(&jdir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "wjl"))
        .unwrap();
    // 模拟写者正写到一半的尾巴
    let mut bytes = std::fs::read(&jpath).unwrap();
    bytes.extend_from_slice(&[0x30, 0, 0, 0, 1, 2]);
    std::fs::write(&jpath, &bytes).unwrap();
    let device = std::fs::read(d.state_dir.join("device.toml")).unwrap();
    // 换了 dir_id 也不能换号封存
    let ro = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: "other".into(),
    })
    .unwrap();
    assert!(ro.readonly());
    assert_eq!(std::fs::read(&jpath).unwrap(), bytes, "只读实例截了日志");
    assert_eq!(
        std::fs::read(d.state_dir.join("device.toml")).unwrap(),
        device
    );
    assert_eq!(texts(&ro, "a"), vec!["甲"], "只读也能读到写者的记录");
}

/// 换号封存：旧日志里未折回的记录仍可见，新日志接着旧号往下编，不混号。
#[test]
fn renewed_device_keeps_unfolded_records_and_continues_numbering() {
    let d = dirs("renew");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "b", "乙");
    up(&ud, "c", "丙");
    drop(ud);
    let d2 = Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: "dir-B".into(),
    };
    let ud = reopen(&d2);
    assert_eq!(texts(&ud, "b"), vec!["乙"], "封存日志里未折回的仍可见");
    assert_eq!(texts(&ud, "c"), vec!["丙"]);
    let seq = ud
        .apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "e",
            "戊",
            Payload::Weight(1),
            0,
        )
        .unwrap();
    assert!(seq > 3, "新日志接着旧号编，实际 {seq}");
    drop(ud);
    let ud = reopen(&d2);
    for (c, t) in [("a", "甲"), ("b", "乙"), ("c", "丙"), ("e", "戊")] {
        assert_eq!(texts(&ud, c), vec![t], "{c}");
    }
}
