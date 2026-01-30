pub fn init_tracing() {
    // Only enable logging when explicitly requested.
    // - stdout is reserved for JSON when `--json` is used
    // - tracing-subscriber defaults to stderr

    let filter = std::env::var("RUST_LOG").ok();
    let enable = filter.as_deref().is_some_and(|s| !s.trim().is_empty())
        || std::env::var_os("MIDI_STUDIO_LOADER_LOG").is_some();

    if !enable {
        return;
    }

    let filter = filter.unwrap_or_else(|| "info".to_string());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init();
}
