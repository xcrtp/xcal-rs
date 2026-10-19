//! X6 验收：不透明句柄 —— 显式 release、跨 Pod 明确报错、数据留在 Pod 内。

use ze_pod::{HandleTable, PodError, PodId};

fn table() -> HandleTable<Vec<f32>> {
    HandleTable::new(PodId::next())
}

/// 验收 1：release 后句柄不可用。
#[test]
fn released_handle_is_unusable() {
    let mut t = table();
    let h = t.insert(vec![1.0, 2.0, 3.0]);
    assert_eq!(t.get(&h).unwrap(), &vec![1.0, 2.0, 3.0]);

    t.release(h).expect("首次释放成功");
    assert_eq!(
        t.get(&h),
        Err(PodError::HandleReleased { slot: h.slot() }),
        "释放后必须明确报错"
    );
    assert!(!t.is_live(&h));
    assert_eq!(t.len(), 0);
}

/// 验收 1（补充）：槽位复用后，**旧句柄**仍然失效（generation 防 ABA）。
#[test]
fn stale_handle_does_not_resurrect() {
    let mut t = table();
    let old = t.insert(vec![1.0]);
    t.release(old).unwrap();
    let new = t.insert(vec![2.0]);

    assert!(t.is_live(&new));
    assert_eq!(
        t.get(&old),
        Err(PodError::HandleReleased { slot: old.slot() }),
        "复用了同一槽位，但旧句柄不得复活"
    );
}

/// 验收 2：跨 Pod 传句柄 ⇒ **明确错误**（不是静默错误）。
#[test]
fn cross_pod_handle_is_explicit_error() {
    let mut a = table();
    let b = table();
    let handle = a.insert(vec![7.0]);

    let err = b.get(&handle).unwrap_err();
    assert!(
        matches!(err, PodError::CrossPodHandle { .. }),
        "跨 Pod 必须是 CrossPodHandle，实际 {err:?}"
    );
    assert!(
        err.to_string().contains("跨 Pod 句柄"),
        "错误信息必须说明是跨 Pod：{err}"
    );

    // 对端表完全没受影响。
    assert_eq!(a.get(&handle).unwrap(), &vec![7.0]);
}

/// 数据留在 Pod 内：外部只持有三个号（owner / slot / generation）。
#[test]
fn handle_is_just_numbers() {
    let mut t = table();
    let h = t.insert(vec![9.0]);
    assert_eq!(h.owner(), t.owner());
    assert_eq!(h.slot(), 0);
    assert_eq!(h.generation(), 0);
}

/// release 两次是明确错误，不是静默成功。
#[test]
fn double_release_is_an_error() {
    let mut t = table();
    let h = t.insert(vec![1.0]);
    t.release(h).unwrap();
    assert!(matches!(t.release(h), Err(PodError::HandleReleased { .. })));
}
