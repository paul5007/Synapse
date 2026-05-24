use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use serde_json::json;
use synapse_core::{Event, EventFilter, EventSource, error_codes};
use synapse_reflex::{
    DEFAULT_MAX_SUBSCRIPTIONS, EVENTS_DROPPED_METRIC, EventBus, EventBusError,
    SUBSCRIBER_QUEUE_CAPACITY,
};

const OVERFLOW_EVENTS: u64 = 5_000;
const EXPECTED_DROPPED: u64 = OVERFLOW_EVENTS - 4_096;

#[test]
fn drop_oldest_5000_events_metric_and_lossy_with_fsv() -> Result<(), Box<dyn Error>> {
    let recorder = TestRecorder::default();
    metrics::with_local_recorder(&recorder, || -> Result<(), Box<dyn Error>> {
        assert_eq!(SUBSCRIBER_QUEUE_CAPACITY, 4_096);
        let bus = EventBus::default();
        let handle = bus.subscribe(EventFilter::All, Vec::new(), false)?;
        println!(
            "source_of_truth=event_bus_queue case=overflow before_count={} before_lossy={} expected_dropped={EXPECTED_DROPPED}",
            handle.len(),
            handle.take_lossy()
        );

        for seq in 0..OVERFLOW_EVENTS {
            let report = bus.publish(event(seq, "tick"));
            assert_eq!(report.matched, 1);
            assert_eq!(report.queued, 1);
        }

        let metric = recorder.counter_value(&metric_key_for(handle.id()))?;
        let queue_len = handle.len();
        let lossy = handle.take_lossy();
        let drained = handle.drain();
        let first_seq = drained.first().map(|event| event.seq);
        let last_seq = drained.last().map(|event| event.seq);
        println!(
            "source_of_truth=event_bus_queue case=overflow after_truth=queue_len:{queue_len} metric:{metric} lossy:{lossy} first_seq:{first_seq:?} last_seq:{last_seq:?} final_value=queue_len={queue_len}"
        );

        assert_eq!(queue_len, SUBSCRIBER_QUEUE_CAPACITY);
        assert_eq!(drained.len(), SUBSCRIBER_QUEUE_CAPACITY);
        assert_eq!(metric, EXPECTED_DROPPED);
        assert!(lossy);
        assert_eq!(first_seq, Some(EXPECTED_DROPPED));
        assert_eq!(last_seq, Some(OVERFLOW_EVENTS - 1));
        Ok(())
    })
}

#[test]
fn subscription_cap_filter_and_unsubscribe_edges_with_fsv() -> Result<(), Box<dyn Error>> {
    let bus = EventBus::default();
    let mut handles = Vec::new();
    for _ in 0..DEFAULT_MAX_SUBSCRIPTIONS {
        handles.push(bus.subscribe(EventFilter::All, Vec::new(), false)?);
    }
    println!(
        "source_of_truth=event_bus_subscriptions case=cap before_count={} cap={DEFAULT_MAX_SUBSCRIPTIONS}",
        bus.subscriber_count()
    );
    let cap_error = match bus.subscribe(EventFilter::All, Vec::new(), false) {
        Ok(_handle) => panic!("65th subscription should fail"),
        Err(error) => error,
    };
    println!(
        "source_of_truth=event_bus_subscriptions case=cap after_truth=code:{} final_value=count:{}",
        cap_error.code(),
        bus.subscriber_count()
    );
    assert_eq!(cap_error.code(), error_codes::SUBSCRIPTION_CAP_REACHED);
    assert_eq!(bus.subscriber_count(), DEFAULT_MAX_SUBSCRIPTIONS);

    let first_id = handles[0].id().to_owned();
    assert!(bus.unsubscribe(&first_id));
    assert!(!bus.unsubscribe(&first_id));
    assert_eq!(bus.subscriber_count(), DEFAULT_MAX_SUBSCRIPTIONS - 1);
    let _report = bus.publish(event(10_000, "tick"));
    assert!(handles[0].is_empty());
    println!(
        "source_of_truth=event_bus_subscriptions case=unsubscribe after_truth=count:{} first_queue:{} final_value=idempotent:true",
        bus.subscriber_count(),
        handles[0].len()
    );

    let filter_bus = EventBus::default();
    let filter_only_handle = filter_bus.subscribe(
        EventFilter::Kind {
            kind: "wanted".to_owned(),
        },
        Vec::new(),
        true,
    )?;
    let kind_list_handle =
        filter_bus.subscribe(EventFilter::All, vec!["allowed".to_owned()], false)?;
    println!(
        "source_of_truth=event_bus_filter case=kind before_kind_queue={} before_kinds_queue={} snapshot_first={}",
        filter_only_handle.len(),
        kind_list_handle.len(),
        filter_only_handle.snapshot_first()
    );
    let _ignored_report = filter_bus.publish(event(1, "ignored"));
    let _wanted_report = filter_bus.publish(event(2, "wanted"));
    let _allowed_report = filter_bus.publish(event(3, "allowed"));
    let filter_only_events = filter_only_handle.drain();
    let kind_list_events = kind_list_handle.drain();
    println!(
        "source_of_truth=event_bus_filter case=kind after_truth=kind_queue:{} kinds_queue:{} final_value=kind_seq:{:?},kinds_seq:{:?}",
        filter_only_events.len(),
        kind_list_events.len(),
        filter_only_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        kind_list_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        filter_only_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        kind_list_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![3]
    );
    Ok(())
}

#[test]
fn invalid_filter_returns_reflex_filter_invalid_with_fsv() {
    let bus = EventBus::default();
    let invalid = EventFilter::And { args: Vec::new() };
    println!("source_of_truth=event_bus_filter case=invalid before=empty_and");
    let result = bus.subscribe(invalid, Vec::new(), false);
    let error = match result {
        Ok(_handle) => panic!("empty AND must fail validation"),
        Err(error) => error,
    };
    println!(
        "source_of_truth=event_bus_filter case=invalid after_truth=code:{} final_value={error:?}",
        error.code()
    );
    assert!(matches!(error, EventBusError::FilterInvalid { .. }));
    assert_eq!(error.code(), error_codes::REFLEX_FILTER_INVALID);
}

fn event(seq: u64, kind: &str) -> Event {
    Event {
        seq,
        at: Utc::now(),
        source: EventSource::System,
        kind: kind.to_owned(),
        data: json!({ "seq": seq, "kind": kind }),
        correlations: Vec::new(),
    }
}

fn metric_key_for(subscription_id: &str) -> String {
    format!("{EVENTS_DROPPED_METRIC}{{subscription_id={subscription_id}}}")
}

#[derive(Clone, Default)]
struct TestRecorder {
    counters: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl TestRecorder {
    fn counter_value(&self, key: &str) -> Result<u64, Box<dyn Error>> {
        let counters = self
            .counters
            .lock()
            .map_err(|error| format!("metric recorder lock poisoned: {error}"))?;
        Ok(counters.get(key).copied().unwrap_or_default())
    }
}

impl Recorder for TestRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        Counter::from_arc(Arc::new(TestCounter {
            key: metric_key(key),
            counters: Arc::clone(&self.counters),
        }))
    }

    fn register_gauge(&self, _key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(Arc::new(NoopGauge))
    }

    fn register_histogram(&self, _key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(Arc::new(NoopHistogram))
    }
}

struct TestCounter {
    key: String,
    counters: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl CounterFn for TestCounter {
    fn increment(&self, value: u64) {
        if let Ok(mut counters) = self.counters.lock() {
            let counter = counters.entry(self.key.clone()).or_default();
            *counter = counter.saturating_add(value);
        }
    }

    fn absolute(&self, value: u64) {
        if let Ok(mut counters) = self.counters.lock() {
            counters.insert(self.key.clone(), value);
        }
    }
}

struct NoopGauge;

impl GaugeFn for NoopGauge {
    fn increment(&self, _value: f64) {}

    fn decrement(&self, _value: f64) {}

    fn set(&self, _value: f64) {}
}

struct NoopHistogram;

impl HistogramFn for NoopHistogram {
    fn record(&self, _value: f64) {}
}

fn metric_key(key: &Key) -> String {
    let mut labels = key
        .labels()
        .map(|label| format!("{}={}", label.key(), label.value()))
        .collect::<Vec<_>>();
    labels.sort();
    format!("{}{{{}}}", key.name(), labels.join(","))
}
