use crate::Args;
use rootcause::prelude::*;
use shared::shutdown::ShutdownSource;

mod config;
mod graph_driver;
mod modules;

pub async fn run(
    Args {
        server_socket_path,
        client_socket_path,
        config: config_path,
    }: Args,
) -> Result<(), Report> {
    // config: load all config
    let config::Config {
        telemetry,
        devices,
        pain,
        credit,
        rest,
        learning,
    } = modules::config::run(config_path)?;
    let learning_cfg = learning.unwrap_or_default();

    // telemetry
    let (telemetry_enabled, telemetry_report_usage) = telemetry
        .map(|t| (t.enabled(), t.report_usage()))
        .unwrap_or_default();
    let telemetry_module = if telemetry_enabled {
        Some(modules::telemetry::init(telemetry_report_usage)?)
    } else {
        tracing::info!("opentelemetry not enabled by config");
        None
    };

    // init endpoints and pain catalogs
    let endpoints_catalog = modules::endpoints::init(devices);
    let (pain_cfg, pain_check_cfg) = pain
        .map(|pain| (Some(pain.settings), pain.check))
        .unwrap_or_default();
    let pain_module = modules::pain::init(pain_cfg, endpoints_catalog.labels())?;
    let pain_check_module = modules::pain_check::init(pain_check_cfg, endpoints_catalog.labels())?;

    // init persistence: load all data
    let (persistence_module, snapshot) =
        modules::persistence::init(pain_module.catalog().labels()).await?;
    let app_state_identity = persistence_module.identity();
    let crate::persistence::AppSnapshot {
        all: initial_all,
        rest: initial_rest,
        breaks: initial_break,
        day: initial_day,
        pain: initial_pain,
        utilization: initial_util,
        activity: initial_activity,
    } = snapshot.unwrap_or_default();

    // shutdown management
    let shutdown = ShutdownSource::new()?;

    // credit utilization
    let config::CreditConfig {
        limits: credit_limits_cfg,
        utilization: credit_util_cfg,
        notifications: credit_notif_cfg,
        costs: credit_costs_cfg,
        rate_boost: credit_rate_boost_cfg,
        global_boost: credit_global_boost_cfg,
    } = credit.unwrap_or_default();
    let (credit_module, credit_calculator) = modules::credit::init(
        credit_limits_cfg,
        credit_util_cfg,
        credit_costs_cfg,
        credit_rate_boost_cfg,
        credit_global_boost_cfg,
    );

    // transports
    let mut binder = crate::integration::Binder::new(endpoints_catalog);
    let pain_sources = pain_module.bind_sources(&mut binder)?;
    let credit_sinks = credit_module.bind_sinks(&mut binder, &initial_util)?;
    let pain_check_bindings = pain_check_module
        .as_ref()
        .map(|module| module.bind_endpoints(&mut binder))
        .transpose()?;
    let label_store = binder.labels();
    let transports_module = modules::transports::init(label_store, binder.complete());

    // activity tracker
    let (activity_producer, activity_module) = modules::activity::init(initial_activity);
    let activity_rx = rest
        .unwrap_or_default()
        .require_no_activity
        .then(|| activity_module.signal_source().subscribe_forward());

    // usage tracking
    let usage_runtime = modules::usage::run(
        initial_all,
        initial_rest,
        initial_break,
        initial_day,
        activity_rx,
    );
    let credit_runtime = credit_module.start(
        usage_runtime.sources().subscribe_forward(),
        initial_util,
        &shutdown,
    );
    let activity_runtime = activity_module.start(usage_runtime.sources().all().subscribe_forward());

    // pain tracking
    let (pain_source, pain_live_source, pain_producer, pain_task) = pain_module.start(initial_pain);
    let pain_check_task = pain_check_module
        .zip(pain_check_bindings)
        .map(|(module, bindings)| {
            module.start(
                pain_source.subscribe_forward(),
                credit_runtime.event_source().subscribe(),
                activity_runtime.state_source().subscribe_forward(),
                bindings,
            )
        });

    // integrations
    let pain_forwarder_task = (!pain_sources.is_empty())
        .then(|| modules::pain_integration::start(pain_sources, pain_producer));
    let sink_forwarder_task = credit_sinks.any().then(|| {
        modules::utilization_integration::start(
            credit_runtime.utilization_source().subscribe_forward(),
            credit_sinks,
        )
    });

    // notifications
    let notification_task = if let Some(cfg) = credit_notif_cfg {
        modules::notifications::run(cfg, credit_runtime.event_source().subscribe())
    } else {
        None
    };

    // flight data recorder
    let fdr_task = learning_cfg.data_recorder.then(|| {
        modules::fdr::run(
            app_state_identity,
            usage_runtime.sources().subscribe_forward(),
            usage_runtime.raw_source().subscribe(),
            pain_source.subscribe_forward(),
            credit_runtime.limit_source().subscribe_forward(),
            activity_runtime.state_source().subscribe_forward(),
            credit_runtime.event_source().subscribe(),
            credit_runtime.utilization_source().subscribe_forward(),
        )
    });

    // run transport backends
    let transport_tasks = transports_module.start(shutdown.signal());

    // run periodic persistence
    let persistence_task = persistence_module.start(
        usage_runtime.sources().subscribe_forward(),
        pain_source.subscribe_forward(),
        credit_runtime.utilization_source().subscribe_forward(),
        activity_runtime.state_source().subscribe_forward(),
    );

    // run server for client IPC
    let ipc_server_task = modules::ipc_server::start(
        client_socket_path,
        usage_runtime.sources().clone(),
        pain_live_source,
        credit_runtime.limit_source().clone(),
        shutdown.signal(),
    )?;

    // feed opentelemetry metrics
    let telemetry_task = telemetry_module.map(|telemetry| {
        telemetry.start(
            usage_runtime.sources().subscribe_forward(),
            pain_source.subscribe_forward(),
            credit_runtime.limit_source().subscribe_forward(),
            activity_runtime.state_source().subscribe_forward(),
        )
    });

    // establish server link
    let (usage_raw_producer, usage_tasks) = usage_runtime.detach();
    let server_link_task = modules::server_link::start(
        server_socket_path,
        usage_raw_producer,
        activity_producer,
        credit_calculator,
        shutdown.signal(),
    );

    // Wait for source/control and derived tasks. Only source/control tasks
    // receive process shutdown directly; derived tasks exit from finite input
    // closure after source/control producers drop.
    let credit_tasks = credit_runtime.detach();
    let mut source_tasks = Vec::new();
    source_tasks.push(credit_tasks.limit_keepalive);
    source_tasks.push(server_link_task);
    source_tasks.extend(transport_tasks);
    source_tasks.push(ipc_server_task);

    let mut transform_tasks = Vec::new();
    transform_tasks.extend(usage_tasks);
    transform_tasks.push(credit_tasks.utilization);
    transform_tasks.push(activity_runtime.detach());
    transform_tasks.push(pain_task);
    if let Some(task) = pain_forwarder_task {
        transform_tasks.push(task);
    }
    if let Some(task) = pain_check_task {
        transform_tasks.push(task);
    }
    if let Some(task) = sink_forwarder_task {
        transform_tasks.push(task);
    }
    if let Some(task) = notification_task {
        transform_tasks.push(task);
    }

    // Sink tasks drain after source/control and transform tasks have dropped the
    // producers that close their inputs.
    let mut sink_tasks = Vec::new();
    sink_tasks.push(persistence_task);
    if let Some(task) = fdr_task {
        sink_tasks.push(task);
    }
    if let Some(task) = telemetry_task {
        sink_tasks.push(task);
    }

    graph_driver::GraphDriver::new(&shutdown, source_tasks, transform_tasks, sink_tasks)
        .run()
        .await
}
