use std::env;
use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use perfetto_sdk::heap_buffer::HeapBuffer;
use perfetto_sdk::pb_msg::{PbMsg, PbMsgWriter};
use perfetto_sdk::protos::config::{
    data_source_config::DataSourceConfig,
    trace_config::{TraceConfig, TraceConfigBufferConfig, TraceConfigDataSource},
    track_event::track_event_config::TrackEventConfig,
};
use perfetto_sdk::tracing_session::TracingSession;
use perfetto_sdk::track_event::{EventContext, TrackEventTrack, TrackEventType};
use tracing::field::{Field, Visit};
use tracing::{Subscriber, span};
use tracing_perfetto_sdk::perfetto_te_ns;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

const TRACE_PATH_ENV: &str = "LOCUSFS_PERFETTO_TRACE";
const TRACE_BUFFER_KB: u32 = 65_536;
const TRACE_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
const PLUGIN_TRACK_PARENT_SEED: u64 = 0x6c6f_6375_7366_7370;

pub(crate) struct PerfettoTraceConfig {
    output: PathBuf,
}

impl PerfettoTraceConfig {
    pub(crate) fn from_env() -> Option<Self> {
        let output = env::var_os(TRACE_PATH_ENV)?;
        Some(Self {
            output: PathBuf::from(output),
        })
    }

    pub(crate) fn output(&self) -> &PathBuf {
        &self.output
    }
}

pub(crate) struct PerfettoTraceSession {
    output: PathBuf,
    session: Option<TracingSession>,
}

struct PluginTrackSpanData {
    name: CString,
    plugin: Option<String>,
    track_name: Option<String>,
    track_id: u64,
}

pub(crate) struct PluginTrackLayer;

impl PluginTrackLayer {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for PluginTrackLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let mut visitor = PluginFieldVisitor::default();
        attrs.values().record(&mut visitor);
        let plugin = visitor.plugin;
        let track_id = plugin.as_deref().map(plugin_track_id).unwrap_or(0);
        let track_name = plugin.as_deref().map(plugin_track_name);
        let name = CString::new(attrs.metadata().name()).unwrap_or_default();
        span.extensions_mut().insert(PluginTrackSpanData {
            name,
            plugin,
            track_name,
            track_id,
        });
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let mut visitor = PluginFieldVisitor::default();
        values.record(&mut visitor);
        let Some(plugin) = visitor.plugin else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(data) = extensions.get_mut::<PluginTrackSpanData>() {
            data.track_id = plugin_track_id(&plugin);
            data.track_name = Some(plugin_track_name(&plugin));
            data.plugin = Some(plugin);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let extensions = span.extensions();
        let Some(data) = extensions.get::<PluginTrackSpanData>() else {
            return;
        };
        let Some(track_name) = data.track_name.as_deref() else {
            return;
        };

        let name_ptr = data.name.as_ptr();
        let track_id = data.track_id;
        perfetto_sdk::track_event!(
            "tracing",
            TrackEventType::SliceBegin(name_ptr),
            |event: &mut EventContext| {
                event.set_named_track_with_dynamic_name(
                    &track_name,
                    track_id,
                    plugin_tracks_parent_uuid(),
                );
            }
        );
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let extensions = span.extensions();
        let Some(data) = extensions.get::<PluginTrackSpanData>() else {
            return;
        };
        let Some(track_name) = data.track_name.as_deref() else {
            return;
        };

        let track_id = data.track_id;
        perfetto_sdk::track_event!(
            "tracing",
            TrackEventType::SliceEnd,
            |event: &mut EventContext| {
                event.set_named_track_with_dynamic_name(
                    &track_name,
                    track_id,
                    plugin_tracks_parent_uuid(),
                );
            }
        );
    }
}

#[derive(Default)]
struct PluginFieldVisitor {
    plugin: Option<String>,
}

impl PluginFieldVisitor {
    fn record_plugin(&mut self, field: &Field, value: impl Into<String>) {
        if field.name() == "plugin" {
            self.plugin = Some(value.into());
        }
    }
}

impl Visit for PluginFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_plugin(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() != "plugin" {
            return;
        }
        let value = format!("{value:?}");
        self.record_plugin(field, value.trim_matches('"'));
    }
}

impl PerfettoTraceSession {
    pub(crate) fn start(config: PerfettoTraceConfig) -> std::io::Result<Self> {
        let mut session = TracingSession::in_process().map_err(std::io::Error::other)?;
        let trace_config = build_trace_config();
        session.setup(&trace_config);
        session.start_blocking();
        Ok(Self {
            output: config.output,
            session: Some(session),
        })
    }
}

impl Drop for PerfettoTraceSession {
    fn drop(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };

        session.flush_blocking(TRACE_FLUSH_TIMEOUT);
        session.stop_blocking();

        let trace_data = Arc::new(Mutex::new(Vec::new()));
        let trace_data_clone = trace_data.clone();
        session.read_trace_blocking(move |data, _has_more| {
            if let Ok(mut trace_data) = trace_data_clone.lock() {
                trace_data.extend_from_slice(data);
            }
        });

        let trace_data = match Arc::try_unwrap(trace_data) {
            Ok(trace_data) => match trace_data.into_inner() {
                Ok(trace_data) => trace_data,
                Err(error) => {
                    eprintln!("locusfs: failed to collect Perfetto trace data: {error}");
                    return;
                }
            },
            Err(_) => {
                eprintln!("locusfs: failed to collect Perfetto trace data");
                return;
            }
        };

        match std::fs::write(&self.output, &trace_data) {
            Ok(()) => eprintln!(
                "locusfs: Perfetto trace written to {} ({} bytes)",
                self.output.display(),
                trace_data.len()
            ),
            Err(error) => eprintln!(
                "locusfs: failed to write Perfetto trace {}: {error}",
                self.output.display()
            ),
        }
    }
}

fn plugin_track_name(plugin: &str) -> String {
    format!("plugin:{plugin}")
}

fn plugin_track_id(plugin: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    PLUGIN_TRACK_PARENT_SEED.hash(&mut hasher);
    plugin.hash(&mut hasher);
    hasher.finish()
}

fn plugin_tracks_parent_uuid() -> u64 {
    TrackEventTrack::named_track_uuid(
        "plugins",
        PLUGIN_TRACK_PARENT_SEED,
        TrackEventTrack::process_track_uuid(),
    )
}

fn build_trace_config() -> Vec<u8> {
    let writer = PbMsgWriter::new();
    let heap_buffer = HeapBuffer::new(writer.stream_writer());
    let mut message = PbMsg::new(&writer).expect("Perfetto trace config message");
    {
        let mut config = TraceConfig { msg: &mut message };
        config.set_buffers(|buffer: &mut TraceConfigBufferConfig| {
            buffer.set_size_kb(TRACE_BUFFER_KB);
        });
        config.set_data_sources(|data_sources: &mut TraceConfigDataSource| {
            data_sources.set_config(|data_source: &mut DataSourceConfig| {
                data_source.set_name("track_event");
                data_source.set_track_event_config(|track_event: &mut TrackEventConfig| {
                    track_event.set_enabled_categories("tracing");
                });
            });
        });
    }
    message.finalize();

    let config_size = writer.stream_writer().get_written_size();
    let mut config_buffer = vec![0u8; config_size];
    heap_buffer.copy_into(&mut config_buffer);
    config_buffer
}
