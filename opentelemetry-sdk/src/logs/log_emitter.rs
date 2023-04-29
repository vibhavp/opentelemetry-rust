use super::{BatchLogProcessor, Config, LogProcessor, LogRuntime, SimpleLogProcessor};
use crate::export::logs::{LogData, LogExporter};
use opentelemetry_api::{
    global::{handle_error, Error},
    logs::{LogRecord, LogResult},
    InstrumentationLibrary,
};
use std::{
    borrow::Cow,
    sync::{Arc, Weak},
};

#[derive(Debug, Clone)]
/// Creator for `LogEmitter` instances.
pub struct LoggerProvider {
    inner: Arc<LoggerProviderInner>,
}

/// Default log emitter name if empty string is provided.
const DEFAULT_COMPONENT_NAME: &str = "rust.opentelemetry.io/sdk/logemitter";

impl LoggerProvider {
    /// Build a new log emitter provider.
    pub(crate) fn new(inner: Arc<LoggerProviderInner>) -> Self {
        LoggerProvider { inner }
    }

    /// Create a new `LogEmitterProvider` builder.
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Create a new versioned `LogEmitter` instance.
    pub fn versioned_log_emitter(
        &self,
        name: impl Into<Cow<'static, str>>,
        version: Option<&'static str>,
        schema_url: Option<Cow<'static, str>>,
        attributes: Option<Vec<opentelemetry_api::KeyValue>>,
    ) -> Logger {
        let name = name.into();

        let component_name = if name.is_empty() {
            Cow::Borrowed(DEFAULT_COMPONENT_NAME)
        } else {
            name
        };

        Logger::new(
            InstrumentationLibrary::new(
                component_name,
                version.map(Into::into),
                schema_url,
                attributes,
            ),
            Arc::downgrade(&self.inner),
        )
    }

    /// Config associated with this provider.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Log processors associated with this provider.
    pub fn log_processors(&self) -> &Vec<Box<dyn LogProcessor>> {
        &self.inner.processors
    }

    /// Force flush all remaining logs in log processors and return results.
    pub fn force_flush(&self) -> Vec<LogResult<()>> {
        self.log_processors()
            .iter()
            .map(|processor| processor.force_flush())
            .collect()
    }

    /// Shuts down this `LogEmitterProvider`, panicking on failure.
    pub fn shutdown(&mut self) -> Vec<LogResult<()>> {
        self.try_shutdown()
            .expect("canont shutdown LogEmitterProvider when child LogEmitters are still active")
    }

    /// Attempts to shutdown this `LogEmitterProvider`, succeeding only when
    /// all cloned `LogEmitterProvider` values have been dropped.
    pub fn try_shutdown(&mut self) -> Option<Vec<LogResult<()>>> {
        Arc::get_mut(&mut self.inner).map(|inner| {
            inner
                .processors
                .iter_mut()
                .map(|processor| processor.shutdown())
                .collect()
        })
    }
}

impl Drop for LoggerProvider {
    fn drop(&mut self) {
        match self.try_shutdown() {
            None => handle_error(Error::Other(
                "canont shutdown LogEmitterProvider when child LogEmitters are still active".into(),
            )),
            Some(results) => {
                for result in results {
                    if let Err(err) = result {
                        handle_error(err)
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct LoggerProviderInner {
    processors: Vec<Box<dyn LogProcessor>>,
    config: Config,
}

#[derive(Debug, Default)]
/// Builder for provider attributes.
pub struct Builder {
    processors: Vec<Box<dyn LogProcessor>>,
    config: Config,
}

impl Builder {
    /// The `LogExporter` that this provider should use.
    pub fn with_simple_exporter<T: LogExporter + 'static>(self, exporter: T) -> Self {
        let mut processors = self.processors;
        processors.push(Box::new(SimpleLogProcessor::new(Box::new(exporter))));

        Builder { processors, ..self }
    }

    /// The `LogExporter` setup using a default `BatchLogProcessor` that this provider should use.
    pub fn with_batch_exporter<T: LogExporter + 'static, R: LogRuntime>(
        self,
        exporter: T,
        runtime: R,
    ) -> Self {
        let batch = BatchLogProcessor::builder(exporter, runtime).build();
        self.with_log_processor(batch)
    }

    /// The `LogProcessor` that this provider should use.
    pub fn with_log_processor<T: LogProcessor + 'static>(self, processor: T) -> Self {
        let mut processors = self.processors;
        processors.push(Box::new(processor));

        Builder { processors, ..self }
    }

    /// The `Config` that this provider should use.
    pub fn with_config(self, config: Config) -> Self {
        Builder { config, ..self }
    }

    /// Create a new provider from this configuration.
    pub fn build(self) -> LoggerProvider {
        LoggerProvider {
            inner: Arc::new(LoggerProviderInner {
                processors: self.processors,
                config: self.config,
            }),
        }
    }
}

#[derive(Debug)]
/// The object for emitting [`LogRecord`]s.
///
/// [`LogRecord`]: crate::log::LogRecord
pub struct Logger {
    instrumentation_lib: InstrumentationLibrary,
    provider: Weak<LoggerProviderInner>,
}

impl Logger {
    pub(crate) fn new(
        instrumentation_lib: InstrumentationLibrary,
        provider: Weak<LoggerProviderInner>,
    ) -> Self {
        Logger {
            instrumentation_lib,
            provider,
        }
    }

    /// LogEmitterProvider associated with this tracer.
    pub fn provider(&self) -> Option<LoggerProvider> {
        self.provider.upgrade().map(LoggerProvider::new)
    }

    /// Instrumentation library information of this tracer.
    pub fn instrumentation_library(&self) -> &InstrumentationLibrary {
        &self.instrumentation_lib
    }

    /// Emit a `LogRecord`.
    pub fn emit(&self, record: LogRecord) {
        let provider = match self.provider() {
            Some(provider) => provider,
            None => return,
        };

        let config = provider.config();
        for processor in provider.log_processors() {
            let data = LogData {
                record: record.clone(),
                resource: config.resource.clone(),
                instrumentation: self.instrumentation_lib.clone(),
            };
            processor.emit(data);
        }
    }
}
