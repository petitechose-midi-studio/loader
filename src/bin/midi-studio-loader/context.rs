use std::time::Duration;

use midi_studio_loader::bridge_control::BridgeControlOptions;

use crate::cli;

pub fn wait_timeout(ms: u64) -> Option<Duration> {
    if ms == 0 {
        None
    } else {
        Some(Duration::from_millis(ms))
    }
}

pub fn bridge_opts(args: &cli::BridgeControlArgs) -> BridgeControlOptions {
    BridgeControlOptions {
        enabled: !args.no_bridge_control,
        service_id: args.bridge_service_id.clone(),
        timeout: Duration::from_millis(args.bridge_timeout_ms),
        control_port: args.bridge_control_port,
        control_timeout: Duration::from_millis(args.bridge_control_timeout_ms),
    }
}
