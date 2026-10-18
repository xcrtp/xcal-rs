//! P6 集成断言：命令 / 事件 / topic / 排序 / 路由 / 确定性。
//!
//! 每条断言都打印**实际输出**（`cargo test -p xc-pod-tests -- --nocapture`）。

use std::sync::Arc;

use xc_pod::{
    Broker, BrokerError, CommandQueue, FlushBoundary, FlushBuffer, OrderedCommand, OrderedEvent,
    Pod, PodHandle, PodId, PodNode, PublishReport, Receipt, SeqKey, Sequenced, publish_topic,
    negotiate_dyn,
};
use xc_pod_tests::{ClockCmd, ClockEvent, ClockPod, ControllerPod, LogPod, StatsPod};

/// 固定 PodId：同源才能得到同键（确定性断言的前提）。
const SOURCE: PodId = PodId::new(7);
const LOG_ID: PodId = PodId::new(11);
const STATS_ID: PodId = PodId::new(22);

fn submit(pod: &Arc<ClockPod>, command: ClockCmd) -> Receipt {
    let queue: CommandQueue<'_, ClockCmd> =
        negotiate_dyn(pod.as_ref()).expect("ClockPod 提供 CommandQueue<ClockCmd>");
    queue.submit(command)
}

fn ticked(frame: u64) -> ClockEvent {
    ClockEvent::Ticked { frame, t: frame }
}

/// 1. `Tick(3)` 与 `Tick(1) × 3` 的事件序列**完全相同**（本任务最重要的一条）。
#[test]
fn tick_batch_equals_repeated_single_ticks() {
    let batch = Arc::new(ClockPod::with_id(SOURCE));
    submit(&batch, ClockCmd::Tick { frames: 3 }).wait();

    let repeated = Arc::new(ClockPod::with_id(SOURCE));
    for _ in 0..3 {
        submit(&repeated, ClockCmd::Tick { frames: 1 }).wait();
    }

    let batch_log = batch.log();
    let repeated_log = repeated.log();

    // 逻辑键（frame, source, seq）逐条相同。
    assert_eq!(batch_log, repeated_log, "逻辑键序列必须完全相同");
    // 负载值也逐条相同（`Sequenced` 的相等性只看键，这里显式比较值）。
    let batch_values: Vec<ClockEvent> =
        batch_log.iter().map(|item| item.value().clone()).collect();
    let repeated_values: Vec<ClockEvent> = repeated_log
        .iter()
        .map(|item| item.value().clone())
        .collect();
    assert_eq!(batch_values, repeated_values);
    assert_eq!(
        batch_values,
        vec![ticked(1), ticked(2), ticked(3)],
        "三帧必须各产出一条事件"
    );
    assert_eq!(batch.frame(), 3);

    eprintln!("[P6-1] Tick(3)     键 = {:?}", keys(&batch_log));
    eprintln!("[P6-1] Tick(1)x3   键 = {:?}", keys(&repeated_log));
    eprintln!("[P6-1] 事件值 = {batch_values:?}");
}

fn keys(items: &[Sequenced<ClockEvent>]) -> Vec<SeqKey> {
    items.iter().map(Sequenced::key).collect()
}

/// 2'. `Ord` 由**框架包装类型**提供（实现方不 derive；键是逻辑序）。
#[test]
fn commands_and_events_are_ordered_by_the_framework_wrapper() {
    fn assert_ord<T: Ord>() {}
    // 命令与事件都经 `Sequenced<T>` 获得全序。
    assert_ord::<OrderedCommand<ClockCmd>>();
    assert_ord::<OrderedEvent<ClockEvent>>();

    // 同一帧、不同来源（PodId）也能定序：先 source 小者，再 seq。
    let source_a = PodHandle::new(PodId::new(1));
    let source_b = PodHandle::new(PodId::new(2));
    let frame_one = SeqKey::new(1, source_a, 9);
    let other_source = SeqKey::new(1, source_b, 0);
    assert!(frame_one < other_source, "同帧时 source 必须参与定序");

    eprintln!("[P6-2b] OrderedCommand<ClockCmd> / OrderedEvent<ClockEvent> 均为 Ord");
}

// ─── 2. 扇出：查询 + 迭代单播，顺序确定 ──────────────────────────────

fn fanout_broker(reverse_declaration: bool) -> (Broker, Arc<LogPod>, Arc<StatsPod>) {
    let log = Arc::new(LogPod::with_id(LOG_ID));
    let stats = Arc::new(StatsPod::with_id(STATS_ID));
    let mut nodes = vec![
        PodNode::new("log", Arc::clone(&log) as Arc<dyn Pod>, &[]).with_topics(&["clock"]),
        PodNode::new("stats", Arc::clone(&stats) as Arc<dyn Pod>, &[]).with_topics(&["clock"]),
    ];
    if reverse_declaration {
        nodes.reverse();
    }
    (Broker::new(nodes), log, stats)
}

#[test]
fn topic_fanout_is_query_driven_and_sorted() {
    for reverse_declaration in [false, true] {
        let (broker, log, stats) = fanout_broker(reverse_declaration);
        let expected_order = vec![PodHandle::new(LOG_ID), PodHandle::new(STATS_ID)];

        // 成员来自 Broker 查询：按 PodId 升序，与节点声明顺序无关。
        assert_eq!(broker.topic_members("clock"), expected_order);
        assert!(broker.topic_members("nobody").is_empty());

        let events = vec![ticked(1), ticked(2)];
        for event in &events {
            let report: PublishReport =
                publish_topic(&broker, "clock", event).expect("扇出成功");
            assert_eq!(report.delivered, 2);
            assert_eq!(report.order, expected_order, "扇出顺序 = 查询顺序");
        }

        assert_eq!(log.entries(), events, "LogPod 收到完整序列");
        assert_eq!(stats.count(), 2);
        assert_eq!(stats.ticks(), vec![1, 2], "StatsPod 与 LogPod 看到同一序列");

        eprintln!(
            "[P6-2] reverse={reverse_declaration} order={:?} log={:?} stats={:?}",
            expected_order,
            log.entries(),
            stats.ticks()
        );
    }
}

/// 声明了 topic 却没有 `EventSink<E>` ⇒ 明确错误（不是静默跳过）。
#[test]
fn topic_member_without_sink_is_an_explicit_error() {
    let clock = Arc::new(ClockPod::with_id(SOURCE));
    let broker = Broker::new(vec![
        PodNode::new("clock", clock as Arc<dyn Pod>, &[]).with_topics(&["clock"]),
    ]);
    match publish_topic(&broker, "clock", &ticked(1)) {
        Err(BrokerError::UnsupportedCapability { node, capability }) => {
            assert_eq!(node, "clock");
            assert!(capability.contains("on_event"), "实际：{capability}");
        }
        other => panic!("期望 UnsupportedCapability，实际 {other:?}"),
    }
}

// ─── 3. 排序：乱序提交后 flush，等于按键排序 ────────────────────────

#[test]
fn flush_sorts_within_explicit_boundary() {
    let source = PodHandle::new(PodId::new(5));
    let mut buffer = FlushBuffer::new();
    // 乱序提交。
    buffer.push(Sequenced::new(SeqKey::new(2, source, 0), 20u32));
    buffer.push(Sequenced::new(SeqKey::new(1, source, 2), 12u32));
    buffer.push(Sequenced::new(SeqKey::new(1, source, 0), 10u32));
    buffer.push(Sequenced::new(SeqKey::new(1, source, 1), 11u32));
    // 逃生舱：不参与排序。
    buffer.push_immediate(99u32);

    let batch = buffer.flush(FlushBoundary::FrameEnd { frame: 2 });
    let actual_keys: Vec<SeqKey> = batch.ordered.iter().map(Sequenced::key).collect();
    let actual_values: Vec<u32> = batch.ordered.iter().map(|item| *item.value()).collect();

    // 显式写出期望键序（不是"把结果再排一遍"）。
    let expected_keys = vec![
        SeqKey::new(1, source, 0),
        SeqKey::new(1, source, 1),
        SeqKey::new(1, source, 2),
        SeqKey::new(2, source, 0),
    ];
    assert_eq!(actual_keys, expected_keys, "flush 后必须等于按键排序");
    assert_eq!(actual_values, vec![10, 11, 12, 20]);
    assert_eq!(batch.immediate, vec![99], "即时通道不参与排序");
    assert_eq!(batch.boundary, FlushBoundary::FrameEnd { frame: 2 });
    assert!(buffer.is_empty());
    eprintln!("[P6-3] flush keys = {actual_keys:?}");
    eprintln!("[P6-3] flush values = {actual_values:?}");
    eprintln!("[P6-3] immediate = {:?}", batch.immediate);

    // 第二种显式边界：Batch 结束。
    let mut buffer = FlushBuffer::new();
    buffer.push(Sequenced::new(SeqKey::new(0, source, 1), 'b'));
    buffer.push(Sequenced::new(SeqKey::new(0, source, 0), 'a'));
    let batch = buffer.flush(FlushBoundary::BatchEnd { command_seq: 3 });
    let chars: Vec<char> = batch.ordered.iter().map(|item| *item.value()).collect();
    assert_eq!(chars, vec!['a', 'b']);
    assert_eq!(batch.boundary, FlushBoundary::BatchEnd { command_seq: 3 });
    eprintln!("[P6-3] BatchEnd values = {chars:?}");
}

// ─── 5. 路由：无 provider / 多 provider / 显式指定 ───────────────────

fn clock_broker(ids: &[u64]) -> Broker {
    Broker::new(
        ids.iter()
            .map(|id| {
                PodNode::new(
                    format!("clock-{id}"),
                    Arc::new(ClockPod::with_id(PodId::new(*id))) as Arc<dyn Pod>,
                    &[],
                )
            })
            .collect(),
    )
}

#[test]
fn routing_errors_are_explicit() {
    let none = Broker::new(vec![]);
    match none.negotiate_unique::<CommandQueue<'_, ClockCmd>>() {
        Err(BrokerError::NoProvider { capability }) => {
            assert!(capability.contains("submit"), "实际：{capability}");
            eprintln!("[P6-5] 无 provider ⇒ NoProvider({capability})");
        }
        other => panic!("期望 NoProvider，实际 {:?}", other.err()),
    }

    let broker = clock_broker(&[1, 2]);
    match broker.negotiate_unique::<CommandQueue<'_, ClockCmd>>() {
        Err(BrokerError::AmbiguousProvider {
            capability,
            providers,
        }) => {
            assert!(capability.contains("submit"));
            assert_eq!(providers, vec!["clock-1".to_string(), "clock-2".to_string()]);
            eprintln!("[P6-5] 多 provider ⇒ AmbiguousProvider({providers:?})");
        }
        other => panic!("期望 AmbiguousProvider，实际 {:?}", other.err()),
    }

    let queue = broker
        .negotiate_on::<CommandQueue<'_, ClockCmd>>("clock-2")
        .expect("显式指定 ⇒ 通过");
    queue.submit(ClockCmd::Tick { frames: 1 }).wait();

    assert!(matches!(
        broker.negotiate_on::<CommandQueue<'_, ClockCmd>>("missing"),
        Err(BrokerError::UnknownNode { .. })
    ));
    eprintln!("[P6-5] 显式指定 clock-2 ⇒ 通过");
}

/// ControllerPod：经 Injector 拿到的 `CommandQueue<ClockCmd>` 提交命令。
#[test]
fn controller_submits_through_injected_queue() {
    let clock = Arc::new(ClockPod::with_id(SOURCE));
    let controller = Arc::new(ControllerPod::new());
    let broker = Broker::new(vec![
        PodNode::new("clock", Arc::clone(&clock) as Arc<dyn Pod>, &["clock"]),
        PodNode::new(
            "controller",
            Arc::clone(&controller) as Arc<dyn Pod>,
            &["controller"],
        ),
    ]);
    let report = broker.assemble().expect("装配 + 注入成功");
    assert_eq!(controller.injected(), vec!["clock".to_string()]);
    assert!(
        report
            .edges
            .contains(&("clock".to_string(), "controller".to_string())),
        "依赖边必须存在：{:?}",
        report.edges
    );

    controller.tick(2).expect("提交成功").wait();
    assert_eq!(clock.events(), vec![ticked(1), ticked(2)]);
    eprintln!(
        "[P6-4] controller 注入={:?} 事件={:?}",
        controller.injected(),
        clock.events()
    );
}

// ─── 6. 确定性：同跑两遍，序列逐条相同 ───────────────────────────────

fn deterministic_run() -> (Vec<ClockEvent>, Vec<PodHandle>, Vec<ClockEvent>) {
    let (broker, log, _stats) = fanout_broker(false);
    let events = vec![ticked(1), ticked(2), ticked(3)];
    let mut order = Vec::new();
    for event in &events {
        let report = publish_topic(&broker, "clock", event).expect("扇出");
        order.push(report.order);
    }
    (events, order.concat(), log.entries())
}

#[test]
fn two_runs_produce_identical_sequences() {
    let first = deterministic_run();
    let second = deterministic_run();
    assert_eq!(first, second, "同跑两遍必须逐条相同");
    eprintln!("[P6-6] run#1 = {first:?}");
    eprintln!("[P6-6] run#2 = {second:?}");
}
