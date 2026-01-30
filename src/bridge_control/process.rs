use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

use super::{BridgeControlError, BridgeControlErrorInfo, BridgePauseInfo, BridgePauseSkipReason};

#[cfg(feature = "process-fallback")]
use super::BridgePauseMethod;

#[derive(Debug, Clone)]
pub(super) struct RelaunchCmd {
    pub(super) exe: PathBuf,
    pub(super) args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OcBridgeProcessInfo {
    pub pid: u32,
    pub exe: Option<String>,
    pub cmd: Option<Vec<String>>,
    pub restartable: bool,
}

#[cfg_attr(not(feature = "process-fallback"), allow(dead_code))]
pub(super) enum ProcessPauseOutcome {
    Paused {
        info: BridgePauseInfo,
        relaunch_cmds: Vec<RelaunchCmd>,
    },
    Skipped(BridgePauseSkipReason),
    Failed(BridgeControlErrorInfo),
}

pub(super) fn resume_processes(cmds: &[RelaunchCmd]) -> Result<(), BridgeControlError> {
    for c in cmds {
        let mut cmd = Command::new(&c.exe);
        cmd.args(&c.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = cmd.spawn().map_err(|e| BridgeControlError::CommandFailed {
            cmd: format!("spawn {:?}", c.exe),
            message: e.to_string(),
        })?;
    }
    Ok(())
}

#[cfg(not(feature = "process-fallback"))]
pub fn list_oc_bridge_processes() -> Vec<OcBridgeProcessInfo> {
    Vec::new()
}

#[cfg(not(feature = "process-fallback"))]
pub(super) fn pause_process_fallback(_timeout: Duration) -> ProcessPauseOutcome {
    // Build without sysinfo process support: we cannot safely stop/relaunch processes.
    ProcessPauseOutcome::Skipped(BridgePauseSkipReason::ProcessNotRestartable)
}

#[cfg(feature = "process-fallback")]
pub fn list_oc_bridge_processes() -> Vec<OcBridgeProcessInfo> {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

    let system = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        ),
    );

    find_oc_bridge_processes(&system)
        .into_iter()
        .map(|p| OcBridgeProcessInfo {
            pid: p.pid_u32,
            exe: p.exe.as_ref().map(|e| e.to_string_lossy().to_string()),
            cmd: p.cmd.clone(),
            restartable: p.exe.is_some(),
        })
        .collect()
}

#[cfg(feature = "process-fallback")]
pub(super) fn pause_process_fallback(timeout: Duration) -> ProcessPauseOutcome {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

    // Process fallback (only if restartable).
    let mut system = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        ),
    );

    let processes = find_oc_bridge_processes(&system);
    if processes.is_empty() {
        return ProcessPauseOutcome::Skipped(BridgePauseSkipReason::NotRunning);
    }

    let mut relaunch_cmds: Vec<RelaunchCmd> = Vec::new();
    let mut pids: Vec<u32> = Vec::new();

    for p in &processes {
        let pid_u32 = p.pid_u32;
        pids.push(pid_u32);

        let Some(exe) = p.exe.clone() else {
            return ProcessPauseOutcome::Skipped(BridgePauseSkipReason::ProcessNotRestartable);
        };

        let args = p
            .cmd
            .clone()
            .unwrap_or_else(|| vec!["--daemon".to_string(), "--no-relaunch".to_string()]);

        relaunch_cmds.push(RelaunchCmd { exe, args });
    }

    // Terminate all oc-bridge processes.
    if let Err(e) = stop_processes(&mut system, &processes, timeout) {
        return ProcessPauseOutcome::Failed(BridgeControlErrorInfo {
            message: format!("unable to stop oc-bridge process: {e}"),
            hint: None,
        });
    }

    let info = BridgePauseInfo {
        method: BridgePauseMethod::Process,
        id: "oc-bridge".to_string(),
        pids,
    };

    ProcessPauseOutcome::Paused {
        info,
        relaunch_cmds,
    }
}

#[cfg(feature = "process-fallback")]
#[derive(Debug, Clone)]
struct OcBridgeProcess {
    pid_u32: u32,
    exe: Option<PathBuf>,
    cmd: Option<Vec<String>>,
}

#[cfg(feature = "process-fallback")]
fn find_oc_bridge_processes(system: &sysinfo::System) -> Vec<OcBridgeProcess> {
    system
        .processes()
        .iter()
        .filter_map(|(pid, p)| {
            let name = p.name();
            if !is_oc_bridge_name(name) {
                return None;
            }

            let exe = match p.exe() {
                Some(e) if !e.as_os_str().is_empty() => Some(e.to_path_buf()),
                _ => None,
            };

            let cmd = {
                let c = p.cmd();
                if c.is_empty() {
                    None
                } else {
                    // sysinfo typically includes argv[0] as the executable. We store argv[1..]
                    // so we can restart from the known exe path.
                    Some(c.iter().skip(1).cloned().collect())
                }
            };

            Some(OcBridgeProcess {
                pid_u32: pid.as_u32(),
                exe,
                cmd,
            })
        })
        .collect()
}

#[cfg(feature = "process-fallback")]
fn is_oc_bridge_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "oc-bridge" || n == "oc-bridge.exe"
}

#[cfg(feature = "process-fallback")]
fn stop_processes(
    system: &mut sysinfo::System,
    procs: &[OcBridgeProcess],
    timeout: Duration,
) -> Result<(), BridgeControlError> {
    // Best-effort: ask processes to exit.
    for p in procs {
        if let Some(proc_) = get_process_by_pid(system, p.pid_u32) {
            let _ = proc_.kill();
        }
    }

    let start = std::time::Instant::now();
    loop {
        system.refresh_processes_specifics(sysinfo::ProcessRefreshKind::new());
        let still_running = procs
            .iter()
            .any(|p| get_process_by_pid(system, p.pid_u32).is_some());
        if !still_running {
            return Ok(());
        }

        if start.elapsed() >= timeout {
            return Err(BridgeControlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(feature = "process-fallback")]
fn get_process_by_pid(system: &sysinfo::System, pid_u32: u32) -> Option<&sysinfo::Process> {
    system.processes().iter().find_map(|(pid, p)| {
        if pid.as_u32() == pid_u32 {
            Some(p)
        } else {
            None
        }
    })
}
