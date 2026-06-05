//! Channel and Message Queue Extraction Pass
//!
//! Detects message passing patterns (Go channels, RabbitMQ, Kafka, Redis pub/sub,
//! EventEmitter) and creates graph edges representing asynchronous data flow.
//!
//! Edge types created:
//! - SENDS_TO: Go channel send (function → channel)
//! - RECEIVES_FROM: Go channel receive (channel → function)
//! - PUBLISHES_TO: Message queue publish (function → topic/queue)
//! - SUBSCRIBES_TO: Message queue subscribe (function → topic/queue)
//! - EMITS: Event emitter emit (function → event)
//! - LISTENS_TO: Event listener registration (function → event)
//!
//! Requirements: 18.1, 18.2, 18.3, 18.4, 18.5

use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::registry::Registry;

/// Placeholder value for dynamic/unresolvable channel/topic/event names.
pub const DYNAMIC_PLACEHOLDER: &str = "<dynamic>";

// ── Go Channel Patterns ──────────────────────────────────────────────────────

/// Detects Go channel send: `ch <- value` or `channelName <- expr`
/// Captures the channel identifier name.
static GO_CHANNEL_SEND_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*<-\s*").unwrap());

/// Detects Go channel receive: `<-ch` or `val := <-channelName`
/// Captures the channel identifier name.
static GO_CHANNEL_RECV_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<-\s*([a-zA-Z_][a-zA-Z0-9_]*)").unwrap());

// ── Message Queue Publish Patterns ───────────────────────────────────────────

/// RabbitMQ publish: basic_publish(..., routing_key="topic") or channel.publish(...)
/// Captures the topic/routing key string literal.
static RABBITMQ_PUBLISH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        # Python/Go: basic_publish or channel.publish with routing_key or exchange
        (?:basic_publish|channel\.publish|\.publish)\s*\([^)]*(?:routing_key|exchange)\s*=\s*["']([^"']+)["'] |
        # Generic: .basic_publish(..., "topic", ...)
        \.basic_publish\s*\([^,]*,\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Kafka producer: producer.send(topic="name") or producer.produce(topic="name")
/// Captures the topic name string literal.
static KAFKA_PUBLISH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        # Python/Java: producer.send("topic", ...) or producer.send(topic="name")
        producer\.send\s*\(\s*(?:topic\s*=\s*)?["']([^"']+)["'] |
        # Confluent: producer.produce\(topic="name"\) or producer.produce\("name"\)
        producer\.produce\s*\(\s*(?:topic\s*=\s*)?["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Redis publish: .publish("channel", message) or redis.publish("channel", ...)
/// Captures the channel name string literal.
static REDIS_PUBLISH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        \.publish\s*\(\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

// ── Message Queue Subscribe Patterns ─────────────────────────────────────────

/// RabbitMQ consume: basic_consume or channel.consume with queue name.
static RABBITMQ_SUBSCRIBE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        (?:basic_consume|channel\.consume|\.consume)\s*\([^)]*(?:queue)\s*=\s*["']([^"']+)["'] |
        \.basic_consume\s*\(\s*(?:queue\s*=\s*)?["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Kafka consumer: consumer.subscribe(["topic"]) or consumer.poll with topic.
static KAFKA_SUBSCRIBE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        # consumer.subscribe(["topic"]) or consumer.subscribe([topic])
        consumer\.subscribe\s*\(\s*\[?\s*["']([^"']+)["'] |
        # consumer.subscribe(topics=["topic"])
        consumer\.subscribe\s*\([^)]*topics?\s*=\s*\[?\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Redis subscribe: .subscribe("channel") or .psubscribe("pattern")
static REDIS_SUBSCRIBE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        \.(?:p?subscribe)\s*\(\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

// ── EventEmitter Patterns ────────────────────────────────────────────────────

/// Node.js EventEmitter emit: .emit("event", ...) or EventEmitter.emit("event")
static EMIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        \.emit\s*\(\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Node.js EventEmitter listener: .on("event", ...) or .addListener("event", ...)
/// Also matches .once("event", ...)
static LISTENER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        \.(?:on|addListener|once)\s*\(\s*["']([^"']+)["']
        "#,
    )
    .unwrap()
});

/// Django signals: signal.send(sender=...) or signal.send_robust(sender=...)
/// Captures the signal variable name from the call context.
static DJANGO_SIGNAL_SEND_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        ([a-zA-Z_][a-zA-Z0-9_]*)\.send(?:_robust)?\s*\(
        "#,
    )
    .unwrap()
});

/// Django/blinker signal connect: signal.connect(handler) or @signal_name.connect
static DJANGO_SIGNAL_LISTEN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        ([a-zA-Z_][a-zA-Z0-9_]*)\.connect\s*\(
        "#,
    )
    .unwrap()
});

// ── Helper Types ─────────────────────────────────────────────────────────────

/// Represents a detected channel/message queue interaction.
#[derive(Debug)]
struct ChannelEdge {
    /// Qualified name of the enclosing function.
    caller_qn: String,
    /// Name of the channel/topic/queue/event.
    channel_name: String,
    /// Type of edge to create.
    edge_type: ChannelEdgeType,
}

#[derive(Debug, Clone, Copy)]
enum ChannelEdgeType {
    SendsTo,
    ReceivesFrom,
    PublishesTo,
    SubscribesTo,
    Emits,
    ListensTo,
}

impl ChannelEdgeType {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendsTo => "SENDS_TO",
            Self::ReceivesFrom => "RECEIVES_FROM",
            Self::PublishesTo => "PUBLISHES_TO",
            Self::SubscribesTo => "SUBSCRIBES_TO",
            Self::Emits => "EMITS",
            Self::ListensTo => "LISTENS_TO",
        }
    }

    /// Returns the node label for the target node type.
    fn target_label(self) -> &'static str {
        match self {
            Self::SendsTo | Self::ReceivesFrom => "Channel",
            Self::PublishesTo | Self::SubscribesTo => "Topic",
            Self::Emits | Self::ListensTo => "Event",
        }
    }

    /// Returns the property name for the edge metadata.
    fn property_name(self) -> &'static str {
        match self {
            Self::SendsTo | Self::ReceivesFrom => "channel",
            Self::PublishesTo | Self::SubscribesTo => "topic",
            Self::Emits | Self::ListensTo => "event",
        }
    }
}

// ── Main Pass Function ───────────────────────────────────────────────────────

/// Channel and Message Queue Extraction Pass.
///
/// Scans source files for message passing patterns and creates edges:
/// - Go channels: SENDS_TO / RECEIVES_FROM
/// - RabbitMQ/Kafka/Redis: PUBLISHES_TO / SUBSCRIBES_TO
/// - EventEmitter/Django signals: EMITS / LISTENS_TO
///
/// Dynamic/unresolvable names use a placeholder value.
pub fn pass_channels(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    let mut channel_nodes_seen: HashSet<String> = HashSet::new();
    let mut edges: Vec<ChannelEdge> = Vec::new();

    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Build line offset table for caller resolution
        let mut line_starts: Vec<usize> = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }

        let file_fns = reg.entries_for_file(&f.rel_path);
        let module_qn = fqn::fqn_module(project, &f.rel_path);

        let ext = f.rel_path.rsplit('.').next().unwrap_or("");

        // Determine which patterns to apply based on file extension
        match ext {
            "go" => {
                detect_go_channels(&source, &line_starts, &file_fns, &module_qn, &mut edges);
            }
            "js" | "ts" | "mjs" | "cjs" | "jsx" | "tsx" => {
                detect_mq_publish(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_mq_subscribe(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_event_emitter(&source, &line_starts, &file_fns, &module_qn, &mut edges);
            }
            "py" => {
                detect_mq_publish(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_mq_subscribe(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_django_signals(&source, &line_starts, &file_fns, &module_qn, &mut edges);
            }
            "java" | "kt" | "kts" | "scala" => {
                detect_mq_publish(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_mq_subscribe(&source, &line_starts, &file_fns, &module_qn, &mut edges);
            }
            "rb" | "rs" | "cs" => {
                detect_mq_publish(&source, &line_starts, &file_fns, &module_qn, &mut edges);
                detect_mq_subscribe(&source, &line_starts, &file_fns, &module_qn, &mut edges);
            }
            _ => {}
        }
    }

    // Create nodes and edges
    let mut edges_created = 0;
    for edge in &edges {
        let target_label = edge.edge_type.target_label();
        let prop_name = edge.edge_type.property_name();
        let channel_qn = format!(
            "{}.{}.{}",
            project,
            target_label.to_lowercase(),
            edge.channel_name
        );

        // Create the channel/topic/event node if not seen yet
        if channel_nodes_seen.insert(channel_qn.clone()) {
            buf.add_node(
                target_label,
                &edge.channel_name,
                &channel_qn,
                "", // no file_path for channel nodes
                0,
                0,
                None,
            );
        }

        // Create the edge with the topic/channel/event name as a property
        let props = serde_json::json!({
            prop_name: edge.channel_name,
        })
        .to_string();

        match edge.edge_type {
            ChannelEdgeType::ReceivesFrom => {
                // RECEIVES_FROM: channel → function
                buf.add_edge_with_confidence(
                    &channel_qn,
                    &edge.caller_qn,
                    edge.edge_type.as_str(),
                    EdgeSource::RegexMatch,
                    Some(props),
                );
            }
            _ => {
                // All others: function → channel/topic/event
                buf.add_edge_with_confidence(
                    &edge.caller_qn,
                    &channel_qn,
                    edge.edge_type.as_str(),
                    EdgeSource::RegexMatch,
                    Some(props),
                );
            }
        }
        edges_created += 1;
    }

    tracing::info!(
        channel_nodes = channel_nodes_seen.len(),
        edges_created = edges_created,
        "pass_channels: complete"
    );
}

// ── Detection Functions ──────────────────────────────────────────────────────

/// Resolve the enclosing function for a match at a given byte offset.
fn resolve_caller(
    mat_start: usize,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
) -> String {
    let line_num = line_starts.partition_point(|&off| off <= mat_start) as i32;
    file_fns
        .iter()
        .rev()
        .find(|e| e.start_line <= line_num && e.end_line >= line_num)
        .map(|e| e.qualified_name.clone())
        .unwrap_or_else(|| module_qn.to_owned())
}

/// Detect Go channel send/receive operations.
fn detect_go_channels(
    source: &str,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
    edges: &mut Vec<ChannelEdge>,
) {
    // Detect sends: `ch <- value`
    for caps in GO_CHANNEL_SEND_RE.captures_iter(source) {
        let channel_name = caps.get(1).unwrap().as_str();
        // Skip common false positives (keywords, common variable names)
        if is_go_channel_false_positive(channel_name) {
            continue;
        }
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: channel_name.to_owned(),
            edge_type: ChannelEdgeType::SendsTo,
        });
    }

    // Detect receives: `<-ch`
    for caps in GO_CHANNEL_RECV_RE.captures_iter(source) {
        let channel_name = caps.get(1).unwrap().as_str();
        if is_go_channel_false_positive(channel_name) {
            continue;
        }
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: channel_name.to_owned(),
            edge_type: ChannelEdgeType::ReceivesFrom,
        });
    }
}

/// Returns true if the identifier is likely a false positive for Go channel detection.
fn is_go_channel_false_positive(name: &str) -> bool {
    matches!(
        name,
        "return" | "if" | "for" | "switch" | "select" | "case" | "default" | "go" | "defer"
    )
}

/// Detect message queue publish patterns (RabbitMQ, Kafka, Redis).
fn detect_mq_publish(
    source: &str,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
    edges: &mut Vec<ChannelEdge>,
) {
    // RabbitMQ publish
    for caps in RABBITMQ_PUBLISH_RE.captures_iter(source) {
        let topic = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: topic,
            edge_type: ChannelEdgeType::PublishesTo,
        });
    }

    // Kafka publish
    for caps in KAFKA_PUBLISH_RE.captures_iter(source) {
        let topic = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: topic,
            edge_type: ChannelEdgeType::PublishesTo,
        });
    }

    // Redis publish
    for caps in REDIS_PUBLISH_RE.captures_iter(source) {
        let topic = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: topic,
            edge_type: ChannelEdgeType::PublishesTo,
        });
    }
}

/// Detect message queue subscribe patterns (RabbitMQ, Kafka, Redis).
fn detect_mq_subscribe(
    source: &str,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
    edges: &mut Vec<ChannelEdge>,
) {
    // RabbitMQ subscribe
    for caps in RABBITMQ_SUBSCRIBE_RE.captures_iter(source) {
        let queue = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: queue,
            edge_type: ChannelEdgeType::SubscribesTo,
        });
    }

    // Kafka subscribe
    for caps in KAFKA_SUBSCRIBE_RE.captures_iter(source) {
        let topic = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: topic,
            edge_type: ChannelEdgeType::SubscribesTo,
        });
    }

    // Redis subscribe
    for caps in REDIS_SUBSCRIBE_RE.captures_iter(source) {
        let channel = extract_first_capture(&caps);
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: channel,
            edge_type: ChannelEdgeType::SubscribesTo,
        });
    }
}

/// Detect Node.js EventEmitter patterns (.emit, .on, .addListener, .once).
fn detect_event_emitter(
    source: &str,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
    edges: &mut Vec<ChannelEdge>,
) {
    // Emit events
    for caps in EMIT_RE.captures_iter(source) {
        let event_name = caps.get(1).unwrap().as_str().to_owned();
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: event_name,
            edge_type: ChannelEdgeType::Emits,
        });
    }

    // Listen for events
    for caps in LISTENER_RE.captures_iter(source) {
        let event_name = caps.get(1).unwrap().as_str().to_owned();
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: event_name,
            edge_type: ChannelEdgeType::ListensTo,
        });
    }
}

/// Detect Django signals and blinker signal patterns.
fn detect_django_signals(
    source: &str,
    line_starts: &[usize],
    file_fns: &[crate::registry::RegistryEntry],
    module_qn: &str,
    edges: &mut Vec<ChannelEdge>,
) {
    // Signal send: signal_name.send(...) or signal_name.send_robust(...)
    for caps in DJANGO_SIGNAL_SEND_RE.captures_iter(source) {
        let signal_name = caps.get(1).unwrap().as_str();
        // Skip common false positives
        if is_django_signal_false_positive(signal_name) {
            continue;
        }
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: signal_name.to_owned(),
            edge_type: ChannelEdgeType::Emits,
        });
    }

    // Signal connect: signal_name.connect(handler)
    for caps in DJANGO_SIGNAL_LISTEN_RE.captures_iter(source) {
        let signal_name = caps.get(1).unwrap().as_str();
        if is_django_signal_false_positive(signal_name) {
            continue;
        }
        let mat_start = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(mat_start, line_starts, file_fns, module_qn);
        edges.push(ChannelEdge {
            caller_qn,
            channel_name: signal_name.to_owned(),
            edge_type: ChannelEdgeType::ListensTo,
        });
    }
}

/// Returns true if the identifier is likely a false positive for Django signal detection.
fn is_django_signal_false_positive(name: &str) -> bool {
    matches!(
        name,
        "self"
            | "cls"
            | "super"
            | "logger"
            | "log"
            | "print"
            | "response"
            | "request"
            | "socket"
            | "client"
            | "conn"
            | "connection"
            | "db"
            | "cursor"
            | "session"
    )
}

/// Extract the first non-None capture group value from a regex match.
/// If no string literal capture is found, returns the DYNAMIC_PLACEHOLDER.
fn extract_first_capture(caps: &regex::Captures) -> String {
    for i in 1..caps.len() {
        if let Some(m) = caps.get(i) {
            let val = m.as_str();
            if !val.is_empty() {
                return val.to_owned();
            }
        }
    }
    DYNAMIC_PLACEHOLDER.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_discover::{DiscoveredFile, Language};
    use codryn_graph_buffer::GraphBuffer;
    use std::io::Write;
    use tempfile::TempDir;

    /// Helper to create a temporary file and return a DiscoveredFile.
    fn write_file(dir: &std::path::Path, rel_path: &str, content: &str) -> DiscoveredFile {
        let abs_path = dir.join(rel_path);
        if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&abs_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let ext = rel_path.rsplit('.').next().unwrap_or("");
        let lang = match ext {
            "go" => Language::Go,
            "js" | "mjs" => Language::JavaScript,
            "ts" => Language::TypeScript,
            "py" => Language::Python,
            "java" => Language::Java,
            _ => Language::Unknown,
        };
        DiscoveredFile {
            abs_path: abs_path.to_path_buf(),
            rel_path: rel_path.to_owned(),
            language: lang,
        }
    }

    #[test]
    fn test_go_channel_send() {
        let dir = TempDir::new().unwrap();
        let content = r#"
package main

func producer(ch chan int) {
    ch <- 42
}
"#;
        let file = write_file(dir.path(), "src/main.go", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Channel node for 'ch'");
        assert!(buf.edge_count() >= 1, "should create SENDS_TO edge");
    }

    #[test]
    fn test_go_channel_receive() {
        let dir = TempDir::new().unwrap();
        let content = r#"
package main

func consumer(ch chan int) {
    val := <-ch
    _ = val
}
"#;
        let file = write_file(dir.path(), "src/main.go", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Channel node for 'ch'");
        assert!(buf.edge_count() >= 1, "should create RECEIVES_FROM edge");
    }

    #[test]
    fn test_kafka_publish() {
        let dir = TempDir::new().unwrap();
        let content = r#"
from kafka import KafkaProducer

def send_order(order):
    producer.send("orders-topic", value=order)
"#;
        let file = write_file(dir.path(), "src/app.py", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Topic node");
        assert!(buf.edge_count() >= 1, "should create PUBLISHES_TO edge");
    }

    #[test]
    fn test_kafka_subscribe() {
        let dir = TempDir::new().unwrap();
        let content = r#"
from kafka import KafkaConsumer

def consume_orders():
    consumer.subscribe(["orders-topic"])
"#;
        let file = write_file(dir.path(), "src/consumer.py", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Topic node");
        assert!(buf.edge_count() >= 1, "should create SUBSCRIBES_TO edge");
    }

    #[test]
    fn test_redis_pubsub() {
        let dir = TempDir::new().unwrap();
        let content = r#"
const redis = require('redis');

function publishEvent(client) {
    client.publish("notifications", JSON.stringify({type: "alert"}));
}

function subscribeEvents(client) {
    client.subscribe("notifications");
}
"#;
        let file = write_file(dir.path(), "src/pubsub.js", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        // Should create 1 Topic node for "notifications" and 2 edges
        assert!(buf.node_count() >= 1, "should create Topic node");
        assert!(
            buf.edge_count() >= 2,
            "should create PUBLISHES_TO and SUBSCRIBES_TO edges"
        );
    }

    #[test]
    fn test_event_emitter() {
        let dir = TempDir::new().unwrap();
        let content = r#"
const EventEmitter = require('events');

class OrderService extends EventEmitter {
    createOrder(data) {
        this.emit("order:created", data);
    }
}

function setupListeners(service) {
    service.on("order:created", handleOrder);
}
"#;
        let file = write_file(dir.path(), "src/orders.js", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Event node");
        assert!(
            buf.edge_count() >= 2,
            "should create EMITS and LISTENS_TO edges"
        );
    }

    #[test]
    fn test_django_signals() {
        let dir = TempDir::new().unwrap();
        let content = r#"
from django.dispatch import Signal

order_completed = Signal()

def complete_order(order):
    order_completed.send(sender=Order, order=order)

def on_order_completed(sender, **kwargs):
    pass

order_completed.connect(on_order_completed)
"#;
        let file = write_file(dir.path(), "src/signals.py", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Event node for signal");
        assert!(
            buf.edge_count() >= 2,
            "should create EMITS and LISTENS_TO edges"
        );
    }

    #[test]
    fn test_rabbitmq_publish_subscribe() {
        let dir = TempDir::new().unwrap();
        let content = r#"
import pika

def publish_message(channel):
    channel.basic_publish(exchange='', routing_key='task_queue', body='Hello')

def consume_messages(channel):
    channel.basic_consume(queue='task_queue', on_message_callback=callback)
"#;
        let file = write_file(dir.path(), "src/mq.py", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert!(buf.node_count() >= 1, "should create Topic node");
        assert!(
            buf.edge_count() >= 2,
            "should create PUBLISHES_TO and SUBSCRIBES_TO edges"
        );
    }

    #[test]
    fn test_no_channels_in_plain_code() {
        let dir = TempDir::new().unwrap();
        let content = r#"
fn main() {
    let x = 42;
    println!("Hello, world!");
}
"#;
        let file = write_file(dir.path(), "src/main.rs", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        assert_eq!(buf.node_count(), 0, "no channel nodes for plain code");
        assert_eq!(buf.edge_count(), 0, "no channel edges for plain code");
    }

    #[test]
    fn test_deduplicates_channel_nodes() {
        let dir = TempDir::new().unwrap();
        let content = r#"
package main

func sender(ch chan int) {
    ch <- 1
    ch <- 2
    ch <- 3
}
"#;
        let file = write_file(dir.path(), "src/main.go", content);
        let reg = Registry::new();
        let mut buf = GraphBuffer::new("p");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_channels(&mut buf, &reg, &files, "p");

        // Should create only 1 Channel node for 'ch' despite 3 sends
        assert_eq!(buf.node_count(), 1, "should deduplicate channel nodes");
        assert_eq!(buf.edge_count(), 3, "should create 3 SENDS_TO edges");
    }
}
