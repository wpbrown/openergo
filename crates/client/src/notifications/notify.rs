pub fn show(body: String) {
    // Fire-and-forget: drop the join handle so the task runs concurrently
    // with the driver loop on the same single-threaded runtime.
    drop(tokio::task::spawn_local(async move {
        if let Err(e) = notify_rust::Notification::new()
            .summary("Openergo")
            .body(&body)
            .show_async()
            .await
        {
            tracing::warn!("Failed to show notification: {e}");
        }
    }));
}
