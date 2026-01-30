use std::time::Duration;

use midi_studio_loader::bridge_control::{BridgeControlMethod, BridgeControlOptions};

use crate::cli;

pub fn wait_timeout(ms: u64) -> Option<Duration> {
    if ms == 0 {
        None
    } else {
        Some(Duration::from_millis(ms))
    }
}

pub fn bridge_opts(args: &cli::BridgeControlArgs) -> BridgeControlOptions {
    let method = if args.no_bridge_control {
        BridgeControlMethod::None
    } else {
        match args.bridge_method {
            cli::BridgeMethodArg::Auto => BridgeControlMethod::Auto,
            cli::BridgeMethodArg::Control => BridgeControlMethod::Control,
            cli::BridgeMethodArg::Service => BridgeControlMethod::Service,
            cli::BridgeMethodArg::Process => BridgeControlMethod::Process,
            cli::BridgeMethodArg::None => BridgeControlMethod::None,
        }
    };

    BridgeControlOptions {
        enabled: !args.no_bridge_control,
        method,
        allow_process_fallback: !args.no_process_fallback,
        service_id: args.bridge_service_id.clone(),
        timeout: Duration::from_millis(args.bridge_timeout_ms),
        control_port: args.bridge_control_port,
        control_timeout: Duration::from_millis(args.bridge_control_timeout_ms),
    }
}
