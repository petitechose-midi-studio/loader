use crate::{
    bridge_control,
    targets::{Target, TargetKind},
};

#[derive(Debug, Clone)]
pub enum OperationEvent {
    /// Target discovery begins.
    DiscoverStart,
    /// A target was observed during discovery.
    TargetDetected {
        index: usize,
        target: Target,
    },
    /// Target discovery finished for this poll.
    DiscoverDone {
        count: usize,
    },
    /// A single target has been chosen for operation.
    TargetSelected {
        target_id: String,
    },

    BridgePauseStart,
    BridgePaused {
        info: bridge_control::BridgePauseInfo,
    },
    BridgePauseSkipped {
        reason: bridge_control::BridgePauseSkipReason,
    },
    BridgePauseFailed {
        error: bridge_control::BridgeControlErrorInfo,
    },
    BridgeResumeStart,
    BridgeResumed,
    BridgeResumeFailed {
        error: bridge_control::BridgeControlErrorInfo,
    },

    HexLoaded {
        bytes: usize,
        blocks: usize,
    },

    /// Operation begins on a target.
    TargetStart {
        target_id: String,
        kind: TargetKind,
    },
    /// Operation finished on a target.
    TargetDone {
        target_id: String,
        ok: bool,
        message: Option<String>,
    },

    SoftReboot {
        target_id: String,
        port: String,
    },
    SoftRebootSkipped {
        target_id: String,
        error: String,
    },
    HalfKayAppeared {
        target_id: String,
        path: String,
    },
    HalfKayOpen {
        target_id: String,
        path: String,
    },

    Block {
        target_id: String,
        index: usize,
        total: usize,
        addr: usize,
    },
    Retry {
        target_id: String,
        addr: usize,
        attempt: u32,
        retries: u32,
        error: String,
    },
    Boot {
        target_id: String,
    },
    Done {
        target_id: String,
    },
}
