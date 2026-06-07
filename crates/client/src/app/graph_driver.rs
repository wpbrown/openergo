use futures::FutureExt;
use futures::future::{Either, select};
use futures::stream::{FuturesUnordered, StreamExt};
use rootcause::prelude::*;
use rootcause::report_collection::ReportCollection;
use shared::shutdown::ShutdownSource;
use shared::spawn::JoinHandle;
use tracing::{error, info};

type Task = JoinHandle<Result<(), Report>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskGroup {
    Source,
    Transform,
    Sink,
}

impl TaskGroup {
    fn label(self) -> &'static str {
        match self {
            TaskGroup::Source => "source task",
            TaskGroup::Transform => "transform task",
            TaskGroup::Sink => "sink task",
        }
    }

    fn is_active(self) -> bool {
        matches!(self, TaskGroup::Source | TaskGroup::Transform)
    }
}

pub struct GraphDriver<'a> {
    shutdown: &'a ShutdownSource,
    source_tasks: Vec<Task>,
    transform_tasks: Vec<Task>,
    sink_tasks: Vec<Task>,
}

impl<'a> GraphDriver<'a> {
    pub fn new(
        shutdown: &'a ShutdownSource,
        source_tasks: Vec<Task>,
        transform_tasks: Vec<Task>,
        sink_tasks: Vec<Task>,
    ) -> Self {
        Self {
            shutdown,
            source_tasks,
            transform_tasks,
            sink_tasks,
        }
    }

    pub async fn run(self) -> Result<(), Report> {
        let mut active_remaining = self.source_tasks.len() + self.transform_tasks.len();
        let mut tasks: FuturesUnordered<_> = self
            .source_tasks
            .into_iter()
            .map(|task| (TaskGroup::Source, task))
            .chain(
                self.transform_tasks
                    .into_iter()
                    .map(|task| (TaskGroup::Transform, task)),
            )
            .chain(
                self.sink_tasks
                    .into_iter()
                    .map(|task| (TaskGroup::Sink, task)),
            )
            .map(|(group, task)| task.join().map(move |joined| (group, joined)))
            .collect();

        let mut shutdown_signal = self.shutdown.signal();
        let mut errors = Vec::new();
        let mut shutting_down = false;

        while !tasks.is_empty() {
            let (group, joined) = if shutting_down || active_remaining == 0 {
                match tasks.next().await {
                    Some(outcome) => outcome,
                    None => break,
                }
            } else {
                let next_task = tasks.next();
                let shutdown_wait = shutdown_signal.wait();
                match select(next_task, shutdown_wait).await {
                    Either::Left((Some(outcome), _)) => outcome,
                    Either::Left((None, _)) => break,
                    Either::Right(((), _)) => {
                        info!("process shutdown observed; waiting for runtime graph to drain");
                        shutting_down = true;
                        continue;
                    }
                }
            };

            if group.is_active() {
                active_remaining -= 1;
            }

            if let Err(report) = joined.result {
                error!(
                    task = joined.name,
                    error = %report,
                    group = group.label(),
                    "task failed; triggering shutdown",
                );
                errors.push(report.context(format!("{}: {}", group.label(), joined.name)));
                if !shutting_down {
                    self.shutdown.trigger();
                    shutting_down = true;
                }
            }
        }

        if errors.is_empty() {
            return Ok(());
        }

        let children: ReportCollection<String> = errors.into_iter().collect();
        Err(children.context("runtime graph failed").into_dynamic())
    }
}
