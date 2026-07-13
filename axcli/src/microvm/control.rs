use std::convert::TryFrom;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::ioctl;

static CONTROL_SERVER: OnceLock<ControlServer> = OnceLock::new();

const HYPERALLOC_POLICY_TARGET_ENV: &str = "AXCLI_HYPERALLOC_POLICY_TARGET_HUGE_FRAMES";
const HYPERALLOC_POLICY_INTERVAL_ENV: &str = "AXCLI_HYPERALLOC_POLICY_INTERVAL_MS";
const HYPERALLOC_POLICY_DEFAULT_INTERVAL_MS: u64 = 2000;
const HYPERALLOC_POLICY_MIN_INTERVAL_MS: u64 = 100;
const HYPERALLOC_DEBUG_RECLAIM_CONTROL_POLL_MS: u64 = 5;
const HYPERALLOC_METRICS_INTERVAL_ENV: &str = "AXCLI_HYPERALLOC_METRICS_INTERVAL_MS";
const HYPERALLOC_METRICS_MIN_INTERVAL_MS: u64 = 100;
const HYPERALLOC_EVAL_INTERVAL_ENV: &str = "AXCLI_HYPERALLOC_EVAL_INTERVAL_MS";
const HYPERALLOC_EVAL_TARGET_ENV: &str = "AXCLI_HYPERALLOC_EVAL_TARGET_HUGE_FRAMES";
const HYPERALLOC_EVAL_DEFAULT_TARGET_HUGE_FRAMES: u64 = 1;
const HYPERALLOC_EVAL_MIN_INTERVAL_MS: u64 = 100;
const HYPERALLOC_SCHEDULER_INTERVAL_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_INTERVAL_MS";
const HYPERALLOC_SCHEDULER_TARGET_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_TARGET_HUGE_FRAMES";
const HYPERALLOC_SCHEDULER_DESIRED_VCPUS_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_DESIRED_VCPUS";
const HYPERALLOC_SCHEDULER_VFIO_QUEUE_GOAL_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_VFIO_QUEUE_GOAL";
const HYPERALLOC_SCHEDULER_BLOCK_IO_WEIGHT_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_BLOCK_IO_WEIGHT";
const HYPERALLOC_SCHEDULER_EQGATE_DRAIN_BUDGET_ENV: &str =
    "AXCLI_HYPERALLOC_SCHEDULER_EQGATE_DRAIN_BUDGET";
const HYPERALLOC_SCHEDULER_ADAPTIVE_ENV: &str = "AXCLI_HYPERALLOC_SCHEDULER_ADAPTIVE";
const HYPERALLOC_SCHEDULER_ADAPTIVE_VFIO_QUEUE_CAP_ENV: &str =
    "AXCLI_HYPERALLOC_SCHEDULER_ADAPTIVE_VFIO_QUEUE_CAP";
const HYPERALLOC_SCHEDULER_MIN_INTERVAL_MS: u64 = 100;
const HYPERALLOC_SCHEDULER_DEFAULT_TARGET_HUGE_FRAMES: u64 = 1;
const HYPERALLOC_SCHEDULER_MAX_VFIO_QUEUE_GOAL: u16 = 1024;
const HYPERALLOC_SCHEDULER_MAX_BLOCK_IO_WEIGHT: u16 = 10000;
const HYPERALLOC_SCHEDULER_MAX_EQGATE_DRAIN_BUDGET: u32 = 1024;
const HYPERALLOC_SCHEDULER_ADAPTIVE_DEFAULT_VFIO_QUEUE_CAP: u16 = 1;
const HYPERALLOC_SCHEDULER_ADAPTIVE_BLOCK_IO_WEIGHT: u16 = 750;
const HYPERALLOC_SCHEDULER_ADAPTIVE_EQGATE_DRAIN_BUDGET: u32 = 16;
const HYPERALLOC_EQGATE_DRAIN_DEFAULT_MAX_REQUESTS: u32 = 16;

struct ControlServer {
    listener: UnixListener,
    socket_path: String,
    instance_fd: i32,
    instance_id: usize,
    has_block_backend: bool,
    has_vfio_backend: bool,
    max_vcpus: u8,
    desired_vcpus: AtomicU8,
    memory_target_huge_frames: AtomicU64,
    hyperalloc_policy: Mutex<Option<HyperAllocPolicy>>,
    hyperalloc_metrics: Mutex<Option<HyperAllocMetricsSampler>>,
    hyperalloc_evaluator: Mutex<Option<HyperAllocPolicyEvaluator>>,
    hyperalloc_scheduler: Mutex<Option<HyperAllocSchedulerDryRun>>,
}

struct HyperAllocPolicy {
    target_huge_frames: u64,
    interval: Duration,
    last_tick: Instant,
    last_issued_seq: u64,
    last_completed_seq: u64,
}

struct HyperAllocMetricsSampler {
    interval: Duration,
    last_sample: Instant,
}

struct HyperAllocPolicyEvaluator {
    interval: Duration,
    last_eval: Instant,
    target_huge_frames: u64,
}

struct HyperAllocSchedulerDryRun {
    interval: Duration,
    last_tick: Instant,
    target_huge_frames: u64,
    desired_vcpus: u8,
    vfio_queue_goal: u16,
    block_io_weight: u16,
    eqgate_drain_budget: u32,
    auto_apply: bool,
    cpu_auto_apply: bool,
    eqgate_auto_drain: bool,
    adaptive: bool,
    adaptive_vfio_queue_cap: u16,
}

struct HyperAllocRuntimeSnapshot {
    desired_vcpus: u8,
    max_vcpus: u8,
    memory_target_huge_frames: u64,
    policy_enabled: u8,
    policy_target_huge_frames: u64,
    policy_interval_ms: u128,
    policy_elapsed_ms: u128,
    policy_last_issued_seq: u64,
    policy_last_completed_seq: u64,
    metrics_enabled: u8,
    metrics_interval_ms: u128,
    metrics_elapsed_ms: u128,
    evaluator_enabled: u8,
    evaluator_interval_ms: u128,
    evaluator_elapsed_ms: u128,
    evaluator_target_huge_frames: u64,
    eval_target_huge_frames: u64,
    scheduler_enabled: u8,
    scheduler_interval_ms: u128,
    scheduler_elapsed_ms: u128,
    scheduler_target_huge_frames: u64,
    scheduler_desired_vcpus: u8,
    scheduler_vfio_queue_goal: u16,
    scheduler_block_io_weight: u16,
    scheduler_eqgate_drain_budget: u32,
    scheduler_auto_apply: u8,
    scheduler_memory_auto_apply: u8,
    scheduler_cpu_auto_apply: u8,
    scheduler_eqgate_auto_drain: u8,
    scheduler_adaptive: u8,
    scheduler_adaptive_vfio_queue_cap: u16,
    has_block_backend: bool,
    has_vfio_backend: bool,
}

struct HyperAllocMemoryTargetResult {
    target_huge_frames: u64,
    sequence: u64,
    status: u32,
    target_pages: u64,
    timeout_ms: u64,
}

struct HyperAllocSchedulerAutoApplyResult {
    result: &'static str,
    sequence: u64,
    status: u32,
    target_pages: u64,
    timeout_ms: u64,
}

struct HyperAllocSchedulerCpuAutoApplyResult {
    result: &'static str,
    desired_before: u8,
    desired_after: u8,
    max_vcpus: u8,
}

struct HyperAllocSchedulerIoApplyResult {
    block_policy_result: &'static str,
    vfio_channel_result: &'static str,
}

struct HyperAllocSchedulerAdaptResult {
    result: &'static str,
    target_before: u64,
    target_after: u64,
    desired_before: u8,
    desired_after: u8,
    vfio_queue_before: u16,
    vfio_queue_after: u16,
    block_weight_before: u16,
    block_weight_after: u16,
    eqgate_budget_before: u32,
    eqgate_budget_after: u32,
}

struct HyperAllocSchedulerEqGateAutoDrainResult {
    result: &'static str,
    flags: u32,
    max_requests: u32,
    visited_pcpus: u64,
    pending_before: u64,
    drained: u64,
    installed: u64,
    unsupported: u64,
    failed: u64,
    pending_after: u64,
    last_sequence: u64,
    skipped: u64,
    blocked_by_other_instance: u64,
}

struct HyperAllocSchedulerMemoryApplyResult {
    result: &'static str,
    sequence: u64,
    status: u32,
    target_pages: u64,
    timeout_ms: u64,
}

struct HyperAllocSchedulerCpuApplyResult {
    result: &'static str,
    desired_before: u8,
    desired_after: u8,
    max_vcpus: u8,
}

unsafe impl Sync for ControlServer {}
unsafe impl Send for ControlServer {}

pub fn start_control_socket(
    instance_fd: i32,
    instance_id: usize,
    default_vcpus: u8,
    max_vcpus: u8,
    has_block_backend: bool,
    has_vfio_backend: bool,
) -> Result<(), String> {
    let socket_path = std::env::var("AXCLI_MICROVM_CONTROL_SOCKET")
        .unwrap_or_else(|_| format!("/tmp/eqvisor-microvm-{}.sock", instance_id));
    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "Failed to remove stale control socket {}: {}",
                socket_path, err
            ));
        }
    }

    let hyperalloc_policy = parse_hyperalloc_policy()?;
    let hyperalloc_metrics = parse_hyperalloc_metrics_sampler()?;
    let hyperalloc_evaluator = parse_hyperalloc_policy_evaluator()?;
    let hyperalloc_scheduler = parse_hyperalloc_scheduler(default_vcpus, max_vcpus)?;
    if has_block_backend {
        let block_io_weight = hyperalloc_scheduler
            .as_ref()
            .map(|scheduler| scheduler.block_io_weight)
            .unwrap_or(0);
        super::block::set_scheduler_block_io_weight(block_io_weight);
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|err| format!("Failed to bind control socket {}: {}", socket_path, err))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))
        .map_err(|err| format!("Failed to chmod control socket {}: {}", socket_path, err))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("Failed to set control socket nonblocking: {}", err))?;

    let server = ControlServer {
        listener,
        socket_path: socket_path.clone(),
        instance_fd,
        instance_id,
        has_block_backend,
        has_vfio_backend,
        max_vcpus,
        desired_vcpus: AtomicU8::new(default_vcpus),
        memory_target_huge_frames: AtomicU64::new(0),
        hyperalloc_policy: Mutex::new(hyperalloc_policy),
        hyperalloc_metrics: Mutex::new(hyperalloc_metrics),
        hyperalloc_evaluator: Mutex::new(hyperalloc_evaluator),
        hyperalloc_scheduler: Mutex::new(hyperalloc_scheduler),
    };

    CONTROL_SERVER
        .set(server)
        .map_err(|_| "microVM control socket already initialized".to_string())?;
    info!(
        "microVM control socket listening path={} instance={} default_vcpus={} max_vcpus={} scheduler_io_backend={}",
        socket_path,
        instance_id,
        default_vcpus,
        max_vcpus,
        hyperalloc_scheduler_io_backend(has_block_backend, has_vfio_backend)
    );
    Ok(())
}

fn parse_hyperalloc_policy() -> Result<Option<HyperAllocPolicy>, String> {
    let target = match std::env::var(HYPERALLOC_POLICY_TARGET_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_POLICY_TARGET_ENV, err))?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(format!("invalid {}: {}", HYPERALLOC_POLICY_TARGET_ENV, err)),
    };
    if target == 0 {
        return Ok(None);
    }

    let mut interval_ms = match std::env::var(HYPERALLOC_POLICY_INTERVAL_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_POLICY_INTERVAL_ENV, err))?,
        Err(std::env::VarError::NotPresent) => HYPERALLOC_POLICY_DEFAULT_INTERVAL_MS,
        Err(err) => {
            return Err(format!(
                "invalid {}: {}",
                HYPERALLOC_POLICY_INTERVAL_ENV, err
            ));
        }
    };
    interval_ms = clamp_hyperalloc_policy_interval_ms(interval_ms);

    info!(
        "HyperAlloc host policy enabled target_huge_frames={} interval_ms={}",
        target, interval_ms
    );
    Ok(Some(HyperAllocPolicy {
        target_huge_frames: target,
        interval: Duration::from_millis(interval_ms),
        last_tick: Instant::now(),
        last_issued_seq: 0,
        last_completed_seq: 0,
    }))
}

fn clamp_hyperalloc_policy_interval_ms(mut interval_ms: u64) -> u64 {
    if interval_ms < HYPERALLOC_POLICY_MIN_INTERVAL_MS {
        warn!(
            "HyperAlloc policy interval {}ms below minimum; clamping to {}ms",
            interval_ms, HYPERALLOC_POLICY_MIN_INTERVAL_MS
        );
        interval_ms = HYPERALLOC_POLICY_MIN_INTERVAL_MS;
    }
    interval_ms
}

fn parse_hyperalloc_metrics_sampler() -> Result<Option<HyperAllocMetricsSampler>, String> {
    let mut interval_ms = match std::env::var(HYPERALLOC_METRICS_INTERVAL_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_METRICS_INTERVAL_ENV, err))?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => {
            return Err(format!(
                "invalid {}: {}",
                HYPERALLOC_METRICS_INTERVAL_ENV, err
            ));
        }
    };
    if interval_ms == 0 {
        return Ok(None);
    }
    interval_ms = clamp_hyperalloc_metrics_interval_ms(interval_ms);

    info!(
        "HyperAlloc metrics sampler enabled interval_ms={}",
        interval_ms
    );
    Ok(Some(HyperAllocMetricsSampler {
        interval: Duration::from_millis(interval_ms),
        last_sample: Instant::now(),
    }))
}

fn clamp_hyperalloc_metrics_interval_ms(mut interval_ms: u64) -> u64 {
    if interval_ms < HYPERALLOC_METRICS_MIN_INTERVAL_MS {
        warn!(
            "HyperAlloc metrics interval {}ms below minimum; clamping to {}ms",
            interval_ms, HYPERALLOC_METRICS_MIN_INTERVAL_MS
        );
        interval_ms = HYPERALLOC_METRICS_MIN_INTERVAL_MS;
    }
    interval_ms
}

fn parse_hyperalloc_policy_evaluator() -> Result<Option<HyperAllocPolicyEvaluator>, String> {
    let mut interval_ms = match std::env::var(HYPERALLOC_EVAL_INTERVAL_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_EVAL_INTERVAL_ENV, err))?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(format!("invalid {}: {}", HYPERALLOC_EVAL_INTERVAL_ENV, err)),
    };
    if interval_ms == 0 {
        return Ok(None);
    }
    interval_ms = clamp_hyperalloc_eval_interval_ms(interval_ms);
    let target_huge_frames = match std::env::var(HYPERALLOC_EVAL_TARGET_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_EVAL_TARGET_ENV, err))?,
        Err(std::env::VarError::NotPresent) => HYPERALLOC_EVAL_DEFAULT_TARGET_HUGE_FRAMES,
        Err(err) => return Err(format!("invalid {}: {}", HYPERALLOC_EVAL_TARGET_ENV, err)),
    };

    info!(
        "HyperAlloc policy evaluator enabled interval_ms={} target_huge_frames={}",
        interval_ms, target_huge_frames
    );
    Ok(Some(HyperAllocPolicyEvaluator {
        interval: Duration::from_millis(interval_ms),
        last_eval: Instant::now(),
        target_huge_frames,
    }))
}

fn clamp_hyperalloc_eval_interval_ms(mut interval_ms: u64) -> u64 {
    if interval_ms < HYPERALLOC_EVAL_MIN_INTERVAL_MS {
        warn!(
            "HyperAlloc policy evaluator interval {}ms below minimum; clamping to {}ms",
            interval_ms, HYPERALLOC_EVAL_MIN_INTERVAL_MS
        );
        interval_ms = HYPERALLOC_EVAL_MIN_INTERVAL_MS;
    }
    interval_ms
}

fn parse_hyperalloc_scheduler(
    default_vcpus: u8,
    max_vcpus: u8,
) -> Result<Option<HyperAllocSchedulerDryRun>, String> {
    let mut interval_ms = match std::env::var(HYPERALLOC_SCHEDULER_INTERVAL_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_SCHEDULER_INTERVAL_ENV, err))?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => {
            return Err(format!(
                "invalid {}: {}",
                HYPERALLOC_SCHEDULER_INTERVAL_ENV, err
            ));
        }
    };
    if interval_ms == 0 {
        return Ok(None);
    }
    interval_ms = clamp_hyperalloc_scheduler_interval_ms(interval_ms);
    let target_huge_frames = match std::env::var(HYPERALLOC_SCHEDULER_TARGET_ENV) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", HYPERALLOC_SCHEDULER_TARGET_ENV, err))?,
        Err(std::env::VarError::NotPresent) => HYPERALLOC_SCHEDULER_DEFAULT_TARGET_HUGE_FRAMES,
        Err(err) => {
            return Err(format!(
                "invalid {}: {}",
                HYPERALLOC_SCHEDULER_TARGET_ENV, err
            ));
        }
    };
    if target_huge_frames == 0 {
        return Err(format!(
            "{} must be nonzero",
            HYPERALLOC_SCHEDULER_TARGET_ENV
        ));
    }
    let desired_vcpus = match std::env::var(HYPERALLOC_SCHEDULER_DESIRED_VCPUS_ENV) {
        Ok(value) => value.parse::<u8>().map_err(|err| {
            format!(
                "invalid {}: {}",
                HYPERALLOC_SCHEDULER_DESIRED_VCPUS_ENV, err
            )
        })?,
        Err(std::env::VarError::NotPresent) => default_vcpus,
        Err(err) => {
            return Err(format!(
                "invalid {}: {}",
                HYPERALLOC_SCHEDULER_DESIRED_VCPUS_ENV, err
            ));
        }
    };
    validate_scheduler_desired_vcpus(desired_vcpus, max_vcpus)?;
    let vfio_queue_goal = parse_scheduler_u16_env(HYPERALLOC_SCHEDULER_VFIO_QUEUE_GOAL_ENV, 0)?;
    let block_io_weight = parse_scheduler_u16_env(HYPERALLOC_SCHEDULER_BLOCK_IO_WEIGHT_ENV, 0)?;
    validate_scheduler_io_goals(vfio_queue_goal, block_io_weight)?;
    let eqgate_drain_budget =
        parse_scheduler_u32_env(HYPERALLOC_SCHEDULER_EQGATE_DRAIN_BUDGET_ENV, 0)?;
    validate_scheduler_eqgate_drain_budget(eqgate_drain_budget)?;
    let adaptive = parse_scheduler_bool_env(HYPERALLOC_SCHEDULER_ADAPTIVE_ENV, false)?;
    let adaptive_vfio_queue_cap = parse_scheduler_adaptive_vfio_queue_cap_env()?;

    info!(
        "HyperAlloc scheduler dry-run enabled interval_ms={} target_huge_frames={} desired_vcpus={} vfio_queue_goal={} block_io_weight={} eqgate_drain_budget={} adaptive={} adaptive_vfio_queue_cap={} auto_apply=0 cpu_auto_apply=0 eqgate_auto_drain=0",
        interval_ms,
        target_huge_frames,
        desired_vcpus,
        vfio_queue_goal,
        block_io_weight,
        eqgate_drain_budget,
        bool_to_u8(adaptive),
        adaptive_vfio_queue_cap
    );
    Ok(Some(HyperAllocSchedulerDryRun {
        interval: Duration::from_millis(interval_ms),
        last_tick: Instant::now(),
        target_huge_frames,
        desired_vcpus,
        vfio_queue_goal,
        block_io_weight,
        eqgate_drain_budget,
        auto_apply: false,
        cpu_auto_apply: false,
        eqgate_auto_drain: false,
        adaptive,
        adaptive_vfio_queue_cap,
    }))
}

fn parse_scheduler_u16_env(name: &str, default_value: u16) -> Result<u16, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|err| format!("invalid {}: {}", name, err)),
        Err(std::env::VarError::NotPresent) => Ok(default_value),
        Err(err) => Err(format!("invalid {}: {}", name, err)),
    }
}

fn parse_scheduler_u32_env(name: &str, default_value: u32) -> Result<u32, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .map_err(|err| format!("invalid {}: {}", name, err)),
        Err(std::env::VarError::NotPresent) => Ok(default_value),
        Err(err) => Err(format!("invalid {}: {}", name, err)),
    }
}

fn parse_scheduler_bool_env(name: &str, default_value: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => match value.as_str() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(format!("invalid {}: must be 0 or 1", name)),
        },
        Err(std::env::VarError::NotPresent) => Ok(default_value),
        Err(err) => Err(format!("invalid {}: {}", name, err)),
    }
}

fn parse_scheduler_adaptive_vfio_queue_cap_env() -> Result<u16, String> {
    let cap = parse_scheduler_u16_env(
        HYPERALLOC_SCHEDULER_ADAPTIVE_VFIO_QUEUE_CAP_ENV,
        HYPERALLOC_SCHEDULER_ADAPTIVE_DEFAULT_VFIO_QUEUE_CAP,
    )?;
    validate_scheduler_adaptive_vfio_queue_cap(cap)?;
    Ok(cap)
}

fn clamp_hyperalloc_scheduler_interval_ms(mut interval_ms: u64) -> u64 {
    if interval_ms < HYPERALLOC_SCHEDULER_MIN_INTERVAL_MS {
        warn!(
            "HyperAlloc scheduler interval {}ms below minimum; clamping to {}ms",
            interval_ms, HYPERALLOC_SCHEDULER_MIN_INTERVAL_MS
        );
        interval_ms = HYPERALLOC_SCHEDULER_MIN_INTERVAL_MS;
    }
    interval_ms
}

fn validate_scheduler_desired_vcpus(desired_vcpus: u8, max_vcpus: u8) -> Result<(), String> {
    if desired_vcpus == 0 || desired_vcpus > max_vcpus {
        return Err(format!(
            "scheduler desired vCPU count {} outside allowed range 1..={}",
            desired_vcpus, max_vcpus
        ));
    }
    Ok(())
}

fn validate_scheduler_io_goals(vfio_queue_goal: u16, block_io_weight: u16) -> Result<(), String> {
    if vfio_queue_goal > HYPERALLOC_SCHEDULER_MAX_VFIO_QUEUE_GOAL {
        return Err(format!(
            "scheduler VFIO queue goal {} outside allowed range 0..={}",
            vfio_queue_goal, HYPERALLOC_SCHEDULER_MAX_VFIO_QUEUE_GOAL
        ));
    }
    if block_io_weight > HYPERALLOC_SCHEDULER_MAX_BLOCK_IO_WEIGHT {
        return Err(format!(
            "scheduler block I/O weight {} outside allowed range 0..={}",
            block_io_weight, HYPERALLOC_SCHEDULER_MAX_BLOCK_IO_WEIGHT
        ));
    }
    Ok(())
}

fn validate_scheduler_adaptive_vfio_queue_cap(cap: u16) -> Result<(), String> {
    if cap > HYPERALLOC_SCHEDULER_MAX_VFIO_QUEUE_GOAL {
        return Err(format!(
            "scheduler adaptive VFIO queue cap {} outside allowed range 0..={}",
            cap, HYPERALLOC_SCHEDULER_MAX_VFIO_QUEUE_GOAL
        ));
    }
    Ok(())
}

fn validate_scheduler_eqgate_drain_budget(budget: u32) -> Result<(), String> {
    if budget > HYPERALLOC_SCHEDULER_MAX_EQGATE_DRAIN_BUDGET {
        return Err(format!(
            "scheduler EqGate drain budget {} outside allowed range 0..={}",
            budget, HYPERALLOC_SCHEDULER_MAX_EQGATE_DRAIN_BUDGET
        ));
    }
    Ok(())
}

pub fn poll_control_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };

    loop {
        match server.listener.accept() {
            Ok((stream, _addr)) => handle_client(server, stream),
            Err(err) if err.kind() == ErrorKind::WouldBlock => break,
            Err(err) => {
                warn!(
                    "microVM control socket accept failed path={}: {}",
                    server.socket_path, err
                );
                break;
            }
        }
    }
}

pub fn poll_policy_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };
    let mut policy_guard = match server.hyperalloc_policy.lock() {
        Ok(policy_guard) => policy_guard,
        Err(err) => {
            warn!("HyperAlloc host policy lock poisoned: {}", err);
            return;
        }
    };
    let Some(policy) = policy_guard.as_mut() else {
        return;
    };
    if policy.last_tick.elapsed() < policy.interval {
        return;
    }
    policy.last_tick = Instant::now();

    let query = match ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64) {
        Ok(query) => query,
        Err(err) => {
            warn!(
                "HyperAlloc host policy query failed target_huge_frames={}: {}",
                policy.target_huge_frames, err
            );
            return;
        }
    };
    if hyperalloc_pagecache_request_outstanding(&query) {
        info!(
            "microVM control memory-target source=policy action=skip reason=pending instance={} last_seq={} pcache_req={} pcache_done={} pcache_failed={}",
            server.instance_id,
            query.last_pagecache_shrink_seq,
            query.pagecache_shrink_pending_requests,
            query.pagecache_shrink_completed_requests,
            query.pagecache_shrink_failed_requests
        );
        return;
    }
    if policy.last_completed_seq != 0
        && query.last_pagecache_shrink_seq == policy.last_completed_seq
    {
        info!(
            "microVM control memory-target source=policy action=skip reason=stable instance={} last_seq={} pcache_req={} pcache_done={} pcache_failed={}",
            server.instance_id,
            query.last_pagecache_shrink_seq,
            query.pagecache_shrink_pending_requests,
            query.pagecache_shrink_completed_requests,
            query.pagecache_shrink_failed_requests
        );
        return;
    }
    if policy.last_issued_seq != 0 && query.last_pagecache_shrink_seq == policy.last_issued_seq {
        policy.last_completed_seq = query.last_pagecache_shrink_seq;
        info!(
            "microVM control memory-target source=policy action=skip reason=completed instance={} last_seq={} pcache_req={} pcache_done={} pcache_failed={}",
            server.instance_id,
            query.last_pagecache_shrink_seq,
            query.pagecache_shrink_pending_requests,
            query.pagecache_shrink_completed_requests,
            query.pagecache_shrink_failed_requests
        );
        return;
    }

    match issue_memory_target(server, policy.target_huge_frames, "policy") {
        Ok(result) => {
            policy.last_issued_seq = result.sequence;
            info!(
                "HyperAlloc host policy issued memory-target instance={} target_huge_frames={} response={}",
                server.instance_id,
                policy.target_huge_frames,
                format_memory_target_response(&result)
            );
        }
        Err(err) => {
            warn!(
                "HyperAlloc host policy memory-target failed target_huge_frames={}: {}",
                policy.target_huge_frames, err
            );
        }
    }
}

pub fn poll_metrics_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };
    let mut metrics_guard = match server.hyperalloc_metrics.lock() {
        Ok(metrics_guard) => metrics_guard,
        Err(err) => {
            warn!("HyperAlloc metrics sampler lock poisoned: {}", err);
            return;
        }
    };
    let Some(metrics) = metrics_guard.as_mut() else {
        return;
    };
    if metrics.last_sample.elapsed() < metrics.interval {
        return;
    }
    metrics.last_sample = Instant::now();

    match ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64) {
        Ok(query) => {
            info!(
                "HyperAlloc metrics instance={} {}",
                server.instance_id,
                format_hyperalloc_status(&query)
            );
        }
        Err(err) => {
            warn!(
                "HyperAlloc metrics sampler query failed instance={}: {}",
                server.instance_id, err
            );
        }
    }
}

pub fn poll_policy_eval_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };
    let mut evaluator_guard = match server.hyperalloc_evaluator.lock() {
        Ok(evaluator_guard) => evaluator_guard,
        Err(err) => {
            warn!("HyperAlloc policy evaluator lock poisoned: {}", err);
            return;
        }
    };
    let Some(evaluator) = evaluator_guard.as_mut() else {
        return;
    };
    if evaluator.last_eval.elapsed() < evaluator.interval {
        return;
    }
    evaluator.last_eval = Instant::now();

    match ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64) {
        Ok(query) => {
            info!(
                "HyperAlloc policy eval instance={} {}",
                server.instance_id,
                format_hyperalloc_policy_eval(&query, evaluator.target_huge_frames)
            );
        }
        Err(err) => {
            warn!(
                "HyperAlloc policy evaluator query failed instance={}: {}",
                server.instance_id, err
            );
        }
    }
}

pub fn poll_scheduler_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };
    {
        let mut scheduler_guard = match server.hyperalloc_scheduler.lock() {
            Ok(scheduler_guard) => scheduler_guard,
            Err(err) => {
                warn!("HyperAlloc scheduler lock poisoned: {}", err);
                return;
            }
        };
        let Some(scheduler) = scheduler_guard.as_mut() else {
            return;
        };
        if scheduler.last_tick.elapsed() < scheduler.interval {
            return;
        }
        scheduler.last_tick = Instant::now();
    };

    let mut snapshot = match hyperalloc_runtime_snapshot(server) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            warn!("HyperAlloc scheduler runtime snapshot failed: {}", err);
            return;
        }
    };
    match ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64) {
        Ok(query) => {
            let adapt = maybe_adapt_hyperalloc_scheduler_goals(server, &snapshot, &query);
            if adapt.result == "updated" {
                match hyperalloc_runtime_snapshot(server) {
                    Ok(updated) => snapshot = updated,
                    Err(err) => warn!(
                        "HyperAlloc scheduler runtime snapshot refresh after adapt failed: {}",
                        err
                    ),
                }
            }
            let (decision, reason) =
                hyperalloc_policy_eval(&query, snapshot.scheduler_target_huge_frames);
            let (memory_action, cpu_action, io_action) =
                hyperalloc_scheduler_actions(&snapshot, decision);
            let auto_apply = maybe_auto_apply_hyperalloc_scheduler(
                server,
                &snapshot,
                decision,
                reason,
                memory_action,
            );
            let cpu_auto_apply =
                maybe_auto_apply_hyperalloc_scheduler_cpu(server, &snapshot, cpu_action);
            let io_apply = maybe_apply_hyperalloc_scheduler_io(&snapshot, io_action);
            let eqgate_auto_drain =
                maybe_auto_drain_hyperalloc_scheduler_eqgate(server, &snapshot, &query);
            info!(
                "HyperAlloc scheduler tick instance={} {}",
                server.instance_id,
                format_hyperalloc_scheduler_tick(
                    &snapshot,
                    &query,
                    decision,
                    reason,
                    memory_action,
                    cpu_action,
                    io_action,
                    &adapt,
                    &auto_apply,
                    &cpu_auto_apply,
                    &io_apply,
                    &eqgate_auto_drain,
                )
            );
        }
        Err(err) => {
            warn!(
                "HyperAlloc scheduler query failed instance={}: {}",
                server.instance_id, err
            );
        }
    }
}

fn handle_client(server: &ControlServer, mut stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut buf = [0u8; 256];
    let request_len = match stream.read(&mut buf) {
        Ok(0) => {
            let _ = writeln!(stream, "ERR empty command");
            return;
        }
        Ok(n) => n,
        Err(err) => {
            let _ = writeln!(stream, "ERR read failed: {}", err);
            return;
        }
    };
    let request = String::from_utf8_lossy(&buf[..request_len]);

    match execute_command(server, request.trim()) {
        Ok(response) => {
            let _ = writeln!(stream, "OK {}", response);
        }
        Err(err) => {
            let _ = writeln!(stream, "ERR {}", err);
        }
    }
}

fn execute_command(server: &ControlServer, request: &str) -> Result<String, String> {
    let mut parts = request.split_whitespace();
    let Some(command) = parts.next() else {
        return Err("empty command".to_string());
    };

    match command {
        "resize-vcpu" | "set-vcpu" => {
            let count = parts
                .next()
                .ok_or_else(|| "missing vCPU count".to_string())?
                .parse::<u8>()
                .map_err(|err| format!("invalid vCPU count: {}", err))?;
            resize_microvm_vcpu(server, count)?;
            Ok(format!(
                "desired_vcpus={} max_vcpus={}",
                count, server.max_vcpus
            ))
        }
        "get-vcpu" | "status" => {
            let desired = server.desired_vcpus.load(Ordering::Acquire);
            let memory_target_huge_frames =
                server.memory_target_huge_frames.load(Ordering::Acquire);
            Ok(format!(
                "desired_vcpus={} max_vcpus={} memory_target_huge_frames={}",
                desired, server.max_vcpus, memory_target_huge_frames
            ))
        }
        "microvm-stop" | "stop" => {
            if parts.next().is_some() {
                return Err("unexpected argument for microvm-stop".to_string());
            }
            stop_microvm_via_control(server)
        }
        "hyperalloc-status" | "ha-status" => {
            let query =
                ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64)?;
            Ok(format_hyperalloc_status(&query))
        }
        "hyperalloc-policy-status" | "ha-policy-status" => format_hyperalloc_policy_status(server),
        "hyperalloc-scheduler-snapshot" | "ha-snapshot" => {
            format_hyperalloc_scheduler_snapshot(server)
        }
        "hyperalloc-scheduler-status" | "ha-scheduler-status" => {
            format_hyperalloc_scheduler_status(server)
        }
        "hyperalloc-scheduler-auto-set" | "ha-scheduler-auto-set" => {
            let enabled = parts
                .next()
                .ok_or_else(|| "missing scheduler auto-apply flag".to_string())?;
            let enabled = match enabled {
                "0" => false,
                "1" => true,
                _ => return Err("scheduler auto-apply flag must be 0 or 1".to_string()),
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler auto-set".to_string());
            }

            set_hyperalloc_scheduler_auto_apply(server, enabled)
        }
        "hyperalloc-scheduler-memory-auto-set" | "ha-scheduler-memory-auto-set" => {
            let enabled = parts
                .next()
                .ok_or_else(|| "missing scheduler memory auto-apply flag".to_string())?;
            let enabled = match enabled {
                "0" => false,
                "1" => true,
                _ => return Err("scheduler memory auto-apply flag must be 0 or 1".to_string()),
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler memory auto-set".to_string());
            }

            set_hyperalloc_scheduler_memory_auto_apply(server, enabled)
        }
        "hyperalloc-scheduler-cpu-auto-set" | "ha-scheduler-cpu-auto-set" => {
            let enabled = parts
                .next()
                .ok_or_else(|| "missing scheduler CPU auto-apply flag".to_string())?;
            let enabled = match enabled {
                "0" => false,
                "1" => true,
                _ => return Err("scheduler CPU auto-apply flag must be 0 or 1".to_string()),
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler CPU auto-set".to_string());
            }

            set_hyperalloc_scheduler_cpu_auto_apply(server, enabled)
        }
        "hyperalloc-scheduler-eqgate-auto-set" | "ha-scheduler-eqgate-auto-set" => {
            let enabled = parts
                .next()
                .ok_or_else(|| "missing scheduler EqGate auto-drain flag".to_string())?;
            let enabled = match enabled {
                "0" => false,
                "1" => true,
                _ => return Err("scheduler EqGate auto-drain flag must be 0 or 1".to_string()),
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler EqGate auto-set".to_string());
            }

            set_hyperalloc_scheduler_eqgate_auto_drain(server, enabled)
        }
        "hyperalloc-scheduler-adaptive-set" | "ha-scheduler-adaptive-set" => {
            let enabled = parts
                .next()
                .ok_or_else(|| "missing scheduler adaptive flag".to_string())?;
            let enabled = match enabled {
                "0" => false,
                "1" => true,
                _ => return Err("scheduler adaptive flag must be 0 or 1".to_string()),
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler adaptive-set".to_string());
            }

            set_hyperalloc_scheduler_adaptive(server, enabled)
        }
        "hyperalloc-scheduler-set" | "ha-scheduler-set" => {
            let interval_ms = parts
                .next()
                .ok_or_else(|| "missing scheduler interval ms".to_string())?
                .parse::<u64>()
                .map_err(|err| format!("invalid scheduler interval ms: {}", err))?;
            let target_huge_frames = match parts.next() {
                Some(value) => Some(value.parse::<u64>().map_err(|err| {
                    format!("invalid scheduler target huge-frame count: {}", err)
                })?),
                None => None,
            };
            let desired_vcpus = match parts.next() {
                Some(value) => Some(
                    value
                        .parse::<u8>()
                        .map_err(|err| format!("invalid scheduler desired vCPU count: {}", err))?,
                ),
                None => None,
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler set".to_string());
            }

            set_hyperalloc_scheduler(server, interval_ms, target_huge_frames, desired_vcpus)
        }
        "hyperalloc-scheduler-io-set" | "ha-scheduler-io-set" => {
            let vfio_queue_goal = parts
                .next()
                .ok_or_else(|| "missing scheduler VFIO queue goal".to_string())?
                .parse::<u16>()
                .map_err(|err| format!("invalid scheduler VFIO queue goal: {}", err))?;
            let block_io_weight = parts
                .next()
                .ok_or_else(|| "missing scheduler block I/O weight".to_string())?
                .parse::<u16>()
                .map_err(|err| format!("invalid scheduler block I/O weight: {}", err))?;
            if parts.next().is_some() {
                return Err("unexpected extra argument for scheduler io-set".to_string());
            }

            set_hyperalloc_scheduler_io(server, vfio_queue_goal, block_io_weight)
        }
        "hyperalloc-scheduler-apply-once" | "ha-scheduler-apply-once" => {
            if parts.next().is_some() {
                return Err("unexpected argument for scheduler apply-once".to_string());
            }
            apply_hyperalloc_scheduler_once(server)
        }
        "hyperalloc-scheduler-apply-all-once" | "ha-scheduler-apply-all-once" => {
            if parts.next().is_some() {
                return Err("unexpected argument for scheduler apply-all-once".to_string());
            }
            apply_hyperalloc_scheduler_all_once(server)
        }
        "hyperalloc-scheduler-cpu-apply-once" | "ha-scheduler-cpu-apply-once" => {
            if parts.next().is_some() {
                return Err("unexpected argument for scheduler CPU apply-once".to_string());
            }
            apply_hyperalloc_scheduler_cpu_once(server)
        }
        "hyperalloc-policy-set" | "ha-policy-set" => {
            let target_huge_frames = parts
                .next()
                .ok_or_else(|| "missing policy target huge-frame count".to_string())?
                .parse::<u64>()
                .map_err(|err| format!("invalid policy target huge-frame count: {}", err))?;
            let interval_ms = match parts.next() {
                Some(value) => Some(
                    value
                        .parse::<u64>()
                        .map_err(|err| format!("invalid policy interval ms: {}", err))?,
                ),
                None => None,
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for policy set".to_string());
            }

            set_hyperalloc_policy(server, target_huge_frames, interval_ms)
        }
        "hyperalloc-metrics-set" | "ha-metrics-set" => {
            let interval_ms = parts
                .next()
                .ok_or_else(|| "missing metrics interval ms".to_string())?
                .parse::<u64>()
                .map_err(|err| format!("invalid metrics interval ms: {}", err))?;
            if parts.next().is_some() {
                return Err("unexpected extra argument for metrics set".to_string());
            }

            set_hyperalloc_metrics(server, interval_ms)
        }
        "hyperalloc-eval-set" | "ha-eval-set" => {
            let interval_ms = parts
                .next()
                .ok_or_else(|| "missing policy eval interval ms".to_string())?
                .parse::<u64>()
                .map_err(|err| format!("invalid policy eval interval ms: {}", err))?;
            let target_huge_frames = match parts.next() {
                Some(value) => Some(value.parse::<u64>().map_err(|err| {
                    format!("invalid policy eval target huge-frame count: {}", err)
                })?),
                None => None,
            };
            if parts.next().is_some() {
                return Err("unexpected extra argument for policy eval set".to_string());
            }

            set_hyperalloc_policy_evaluator(server, interval_ms, target_huge_frames)
        }
        "hyperalloc-eval" | "ha-eval" => {
            let target_huge_frames = match parts.next() {
                Some(value) => value.parse::<u64>().map_err(|err| {
                    format!("invalid policy eval target huge-frame count: {}", err)
                })?,
                None => HYPERALLOC_EVAL_DEFAULT_TARGET_HUGE_FRAMES,
            };
            let query =
                ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64)?;
            Ok(format_hyperalloc_policy_eval(&query, target_huge_frames))
        }
        "hyperalloc-vfio-dma-debug" | "ha-vfio-dma-debug" => {
            issue_hyperalloc_vfio_dma_debug_request(server, &mut parts)
        }
        "hyperalloc-vfio-dma-complete" | "ha-vfio-dma-complete" => {
            issue_hyperalloc_vfio_dma_complete_request(server, &mut parts)
        }
        "hyperalloc-debug-reclaim" | "ha-debug-reclaim" => {
            issue_hyperalloc_debug_reclaim(server, &mut parts)
        }
        "microvm-guest-ram-mmap-zap" | "ha-vma-zap" => {
            issue_microvm_guest_ram_mmap_zap(server, &mut parts)
        }
        "hyperalloc-eqgate-enqueue-debug" | "ha-eqgate-enqueue-debug" => {
            issue_hyperalloc_eqgate_debug_enqueue(server, &mut parts)
        }
        "hyperalloc-eqgate-drain" | "ha-eqgate-drain" => {
            issue_hyperalloc_eqgate_drain(server, &mut parts)
        }
        "hyperalloc-scheduler-eqgate-drain-once" | "ha-scheduler-eqgate-drain-once" => {
            issue_hyperalloc_scheduler_eqgate_drain_once(server, &mut parts)
        }
        "memory-target" | "set-memory-target" => {
            let target_huge_frames = parts
                .next()
                .ok_or_else(|| "missing memory target huge-frame count".to_string())?
                .parse::<u64>()
                .map_err(|err| format!("invalid memory target huge-frame count: {}", err))?;
            if target_huge_frames == 0 {
                return Err("memory target huge-frame count must be nonzero".to_string());
            }

            issue_memory_target(server, target_huge_frames, "manual")
                .map(|result| format_memory_target_response(&result))
        }
        _ => Err(format!("unknown command '{}'", command)),
    }
}

fn stop_microvm_via_control(server: &ControlServer) -> Result<String, String> {
    let result = ioctl::ioctl_microvm_stop(server.instance_fd, server.instance_id as u64)?;

    info!(
        "microVM control stop instance={} active_pcpus_signalled={}",
        server.instance_id, result
    );
    Ok(format!(
        "microvm_stop instance={} active_pcpus_signalled={}",
        server.instance_id, result
    ))
}

fn ioctl_hyperalloc_debug_reclaim_with_runtime_poll(
    instance_fd: i32,
    instance_id: u64,
    zone_id: u32,
    frame_gpa: u64,
    frame_len: u64,
    flags: u32,
) -> Result<eqvm_defs::EqHyperAllocDebugReclaimReq, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("ha-debug-reclaim-ioctl".to_string())
        .spawn(move || {
            let result = ioctl::ioctl_hyperalloc_debug_reclaim(
                instance_fd,
                instance_id,
                zone_id,
                frame_gpa,
                frame_len,
                flags,
            );
            let _ = tx.send(result);
        })
        .map_err(|err| format!("Failed to spawn HyperAlloc debug reclaim helper: {}", err))?;

    let start = Instant::now();
    let mut runtime_polls = 0u64;
    loop {
        match rx.try_recv() {
            Ok(result) => {
                if worker.join().is_err() {
                    return Err("HyperAlloc debug reclaim helper panicked".to_string());
                }
                if runtime_polls != 0 {
                    info!(
                        "microVM control HyperAlloc debug reclaim runtime-poll helper instance={} polls={} elapsed_ms={}",
                        instance_id,
                        runtime_polls,
                        start.elapsed().as_millis()
                    );
                }
                return result;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = worker.join();
                return Err("HyperAlloc debug reclaim helper disconnected".to_string());
            }
        }

        if super::vfio_runtime::poll_hyperalloc_runtime_once_for_control() {
            runtime_polls = runtime_polls.saturating_add(1);
        }
        thread::sleep(Duration::from_millis(
            HYPERALLOC_DEBUG_RECLAIM_CONTROL_POLL_MS,
        ));
    }
}

fn issue_hyperalloc_debug_reclaim<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let frame_gpa = match parts.next() {
        Some(value) => parse_u64_arg(value, "HyperAlloc debug reclaim frame GPA")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    let zone_id = match parts.next() {
        Some(value) => parse_u32_arg(value, "HyperAlloc debug reclaim zone id")?,
        None => 0,
    };
    let frame_len = match parts.next() {
        Some(value) => parse_u64_arg(value, "HyperAlloc debug reclaim frame length")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    let mut flags = 0u32;
    for value in parts {
        match value {
            "--prezapped" | "prezapped" => {
                flags |= eqvm_defs::EQ_HYPERALLOC_DEBUG_RECLAIM_FLAG_GUEST_RAM_PREZAPPED;
            }
            _ => return Err("unexpected extra argument for HyperAlloc debug reclaim".to_string()),
        }
    }

    let req = ioctl_hyperalloc_debug_reclaim_with_runtime_poll(
        server.instance_fd,
        server.instance_id as u64,
        zone_id,
        frame_gpa,
        frame_len,
        flags,
    )?;
    info!(
        "microVM control HyperAlloc debug reclaim instance={} flags={:#x} zone={} frame_gpa={:#x} frame_len={:#x} installed_after={} soft_after={} hard_after={} physical_releases_after={} physically_released_after={} last_physical_release_hpa={:#x} errno={}",
        server.instance_id,
        req.flags,
        req.zone_id,
        req.frame_gpa,
        req.frame_len,
        req.installed_after,
        req.soft_after,
        req.hard_after,
        req.physical_releases_after,
        req.physically_released_after,
        req.last_physical_release_hpa,
        req.result_errno
    );
    Ok(format!(
        "debug_reclaim instance={} flags={:#x} zone={} frame_gpa={:#x} frame_len={:#x} installed_after={} soft_after={} hard_after={} physical_releases_after={} physically_released_after={} last_physical_release_hpa={:#x} errno={}",
        req.instance_id,
        req.flags,
        req.zone_id,
        req.frame_gpa,
        req.frame_len,
        req.installed_after,
        req.soft_after,
        req.hard_after,
        req.physical_releases_after,
        req.physically_released_after,
        req.last_physical_release_hpa,
        req.result_errno
    ))
}

fn issue_microvm_guest_ram_mmap_zap<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let gpa = match parts.next() {
        Some(value) => parse_u64_arg(value, "MicroVM guest RAM mmap zap GPA")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    let len = match parts.next() {
        Some(value) => parse_u64_arg(value, "MicroVM guest RAM mmap zap length")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    if len == 0 {
        return Err("MicroVM guest RAM mmap zap length must be nonzero".to_string());
    }
    if parts.next().is_some() {
        return Err("unexpected extra argument for MicroVM guest RAM mmap zap".to_string());
    }

    let req = ioctl::ioctl_microvm_guest_ram_mmap_zap(
        server.instance_fd,
        server.instance_id as u64,
        gpa,
        len,
    )?;
    info!(
        "microVM control guest RAM mmap zap instance={} gpa={:#x} len={:#x} zapped_vmas={} zapped_bytes={:#x} errno={}",
        server.instance_id, req.gpa, req.len, req.zapped_vmas, req.zapped_bytes, req.result_errno
    );
    Ok(format!(
        "vma_zap instance={} gpa={:#x} len={:#x} zapped_vmas={} zapped_bytes={:#x} errno={}",
        req.instance_id, req.gpa, req.len, req.zapped_vmas, req.zapped_bytes, req.result_errno
    ))
}

fn issue_hyperalloc_eqgate_debug_enqueue<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let frame_gpa = match parts.next() {
        Some(value) => parse_u64_arg(value, "EqGate debug enqueue frame GPA")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    let zone_id = match parts.next() {
        Some(value) => parse_u32_arg(value, "EqGate debug enqueue zone id")?,
        None => 0,
    };
    let vcpu_id = match parts.next() {
        Some(value) => parse_u32_arg(value, "EqGate debug enqueue vCPU id")?,
        None => 0,
    };
    let frame_len = match parts.next() {
        Some(value) => parse_u64_arg(value, "EqGate debug enqueue frame length")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    let entry_flags = match parts.next() {
        Some(value) => parse_u64_arg(value, "EqGate debug enqueue entry flags")?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err("unexpected extra argument for EqGate debug enqueue".to_string());
    }

    let req = ioctl::ioctl_hyperalloc_eqgate_debug_enqueue(
        server.instance_fd,
        server.instance_id as u64,
        vcpu_id,
        zone_id,
        frame_gpa,
        frame_len,
        entry_flags,
    )?;
    info!(
        "microVM control HyperAlloc EqGate debug enqueue instance={} vcpu={} zone={} frame_gpa={:#x} frame_len={:#x} target_pcpu={} sequence={} pending_before={} pending_after={} submitted_after={} dropped_after={} errno={}",
        server.instance_id,
        req.vcpu_id,
        req.zone_id,
        req.frame_gpa,
        req.frame_len,
        req.target_pcpu,
        req.sequence,
        req.pending_before,
        req.pending_after,
        req.submitted_after,
        req.dropped_after,
        req.result_errno
    );
    Ok(format!(
        "eqgate_debug_enqueue instance={} vcpu={} zone={} frame_gpa={:#x} frame_len={:#x} entry_flags={:#x} target_pcpu={} sequence={} pending_before={} pending_after={} submitted_after={} dropped_after={} errno={}",
        req.instance_id,
        req.vcpu_id,
        req.zone_id,
        req.frame_gpa,
        req.frame_len,
        req.entry_flags,
        req.target_pcpu,
        req.sequence,
        req.pending_before,
        req.pending_after,
        req.submitted_after,
        req.dropped_after,
        req.result_errno
    ))
}

fn issue_hyperalloc_eqgate_drain<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let mut max_requests = HYPERALLOC_EQGATE_DRAIN_DEFAULT_MAX_REQUESTS;
    let mut max_requests_seen = false;
    let mut flags = 0u32;

    for value in parts {
        match value {
            "--execute" | "execute" => {
                flags |= eqvm_defs::EQ_HYPERALLOC_EQGATE_DRAIN_FLAG_EXECUTE;
            }
            value => {
                if max_requests_seen {
                    return Err("unexpected extra argument for EqGate drain".to_string());
                }
                let parsed = parse_u64_arg(value, "EqGate drain max_requests")?;
                max_requests = u32::try_from(parsed).map_err(|_| {
                    format!(
                        "invalid EqGate drain max_requests: {} outside u32 range",
                        value
                    )
                })?;
                max_requests_seen = true;
            }
        }
    }
    if max_requests == 0 {
        return Err("EqGate drain max_requests must be nonzero".to_string());
    }

    let req = ioctl::ioctl_hyperalloc_eqgate_drain(
        server.instance_fd,
        server.instance_id as u64,
        max_requests,
        flags,
    )?;
    info!(
        "microVM control HyperAlloc EqGate drain instance={} flags={:#x} max_requests={} visited_pcpus={} pending_before={} drained={} installed={} unsupported={} failed={} pending_after={} last_seq={} errno={} skipped={} blocked_by_other_instance={}",
        server.instance_id,
        req.flags,
        req.max_requests,
        req.visited_pcpus,
        req.pending_before,
        req.drained,
        req.installed,
        req.unsupported,
        req.failed,
        req.pending_after,
        req.last_sequence,
        req.result_errno,
        req.skipped,
        req.blocked_by_other_instance
    );
    Ok(format!(
        "eqgate_drain instance={} flags={:#x} max_requests={} visited_pcpus={} pending_before={} drained={} installed={} unsupported={} failed={} pending_after={} last_seq={} errno={} skipped={} blocked_by_other_instance={}",
        req.instance_id,
        req.flags,
        req.max_requests,
        req.visited_pcpus,
        req.pending_before,
        req.drained,
        req.installed,
        req.unsupported,
        req.failed,
        req.pending_after,
        req.last_sequence,
        req.result_errno,
        req.skipped,
        req.blocked_by_other_instance
    ))
}

fn issue_hyperalloc_scheduler_eqgate_drain_once<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let snapshot = hyperalloc_runtime_snapshot(server)?;
    if snapshot.scheduler_enabled == 0 {
        return Err("HyperAlloc scheduler dry-run is disabled".to_string());
    }
    let mut max_requests = snapshot.scheduler_eqgate_drain_budget;
    let mut max_requests_seen = false;
    let mut execute = false;

    for value in parts {
        match value {
            "--execute" | "execute" => {
                execute = true;
            }
            value => {
                if max_requests_seen {
                    return Err(
                        "unexpected extra argument for scheduler EqGate drain once".to_string()
                    );
                }
                let parsed = parse_u64_arg(value, "scheduler EqGate drain max_requests")?;
                max_requests = u32::try_from(parsed).map_err(|_| {
                    format!(
                        "invalid scheduler EqGate drain max_requests: {} outside u32 range",
                        value
                    )
                })?;
                max_requests_seen = true;
            }
        }
    }

    if max_requests == 0 {
        return Err("scheduler EqGate drain max_requests must be nonzero".to_string());
    }

    let flags = if execute {
        eqvm_defs::EQ_HYPERALLOC_EQGATE_DRAIN_FLAG_EXECUTE
    } else {
        0
    };
    let req = ioctl::ioctl_hyperalloc_eqgate_drain(
        server.instance_fd,
        server.instance_id as u64,
        max_requests,
        flags,
    )?;
    info!(
        "microVM control HyperAlloc scheduler EqGate drain once instance={} execute={} scheduler_eqgate_drain_budget={} flags={:#x} max_requests={} visited_pcpus={} pending_before={} drained={} installed={} unsupported={} failed={} pending_after={} last_seq={} errno={} skipped={} blocked_by_other_instance={}",
        server.instance_id,
        execute,
        snapshot.scheduler_eqgate_drain_budget,
        req.flags,
        req.max_requests,
        req.visited_pcpus,
        req.pending_before,
        req.drained,
        req.installed,
        req.unsupported,
        req.failed,
        req.pending_after,
        req.last_sequence,
        req.result_errno,
        req.skipped,
        req.blocked_by_other_instance
    );
    Ok(format!(
        "scheduler_eqgate_drain_once instance={} execute={} scheduler_eqgate_drain_budget={} flags={:#x} max_requests={} visited_pcpus={} pending_before={} drained={} installed={} unsupported={} failed={} pending_after={} last_seq={} errno={} skipped={} blocked_by_other_instance={}",
        server.instance_id,
        execute,
        snapshot.scheduler_eqgate_drain_budget,
        req.flags,
        req.max_requests,
        req.visited_pcpus,
        req.pending_before,
        req.drained,
        req.installed,
        req.unsupported,
        req.failed,
        req.pending_after,
        req.last_sequence,
        req.result_errno,
        req.skipped,
        req.blocked_by_other_instance
    ))
}

fn issue_hyperalloc_vfio_dma_debug_request<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let op = match parts
        .next()
        .ok_or_else(|| "missing VFIO DMA op".to_string())?
    {
        "map" => eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_MAP,
        "unmap" => eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_UNMAP,
        _ => return Err("VFIO DMA op must be map or unmap".to_string()),
    };
    let frame_gpa = parse_u64_arg(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA frame GPA".to_string())?,
        "VFIO DMA frame GPA",
    )?;
    let frame_hpa = parse_u64_arg(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA frame HPA".to_string())?,
        "VFIO DMA frame HPA",
    )?;
    let iova = match parts.next() {
        Some(value) => parse_u64_arg(value, "VFIO DMA IOVA")?,
        None => frame_gpa,
    };
    let size = match parts.next() {
        Some(value) => parse_u64_arg(value, "VFIO DMA size")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    if parts.next().is_some() {
        return Err("unexpected extra argument for VFIO DMA debug request".to_string());
    }

    let mut op_req = eqvm_defs::EqHyperAllocVfioDmaOp {
        version: eqvm_defs::EQ_HYPERALLOC_VERSION,
        op,
        status: eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING,
        instance_id: server.instance_id as u64,
        frame_gpa,
        frame_hpa,
        iova,
        size,
        ..eqvm_defs::EqHyperAllocVfioDmaOp::default()
    };
    ioctl::ioctl_hyperalloc_vfio_dma_debug_request(server.instance_fd, &mut op_req)?;
    info!(
        "microVM control HyperAlloc VFIO DMA debug request instance={} seq={} op={} iova={:#x} hpa={:#x} size={:#x}",
        server.instance_id,
        op_req.sequence,
        hyperalloc_vfio_dma_op_name(op_req.op),
        op_req.iova,
        op_req.frame_hpa,
        op_req.size
    );
    Ok(format!(
        "vfio_dma_debug seq={} op={} op_name={} status={} status_name={} frame_gpa={:#x} frame_hpa={:#x} iova={:#x} size={:#x} errno={}",
        op_req.sequence,
        op_req.op,
        hyperalloc_vfio_dma_op_name(op_req.op),
        op_req.status,
        hyperalloc_vfio_dma_status_name(op_req.status),
        op_req.frame_gpa,
        op_req.frame_hpa,
        op_req.iova,
        op_req.size,
        op_req.result_errno
    ))
}

fn issue_hyperalloc_vfio_dma_complete_request<'a, I>(
    server: &ControlServer,
    parts: &mut I,
) -> Result<String, String>
where
    I: Iterator<Item = &'a str>,
{
    let sequence = parse_u64_arg(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA completion sequence".to_string())?,
        "VFIO DMA completion sequence",
    )?;
    let op = match parts
        .next()
        .ok_or_else(|| "missing VFIO DMA completion op".to_string())?
    {
        "map" => eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_MAP,
        "unmap" => eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_UNMAP,
        _ => return Err("VFIO DMA completion op must be map or unmap".to_string()),
    };
    let status = parse_hyperalloc_vfio_dma_status(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA completion status".to_string())?,
    )?;
    let frame_gpa = parse_u64_arg(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA completion frame GPA".to_string())?,
        "VFIO DMA completion frame GPA",
    )?;
    let frame_hpa = parse_u64_arg(
        parts
            .next()
            .ok_or_else(|| "missing VFIO DMA completion frame HPA".to_string())?,
        "VFIO DMA completion frame HPA",
    )?;
    let iova = match parts.next() {
        Some(value) => parse_u64_arg(value, "VFIO DMA completion IOVA")?,
        None => frame_gpa,
    };
    let size = match parts.next() {
        Some(value) => parse_u64_arg(value, "VFIO DMA completion size")?,
        None => eqvm_defs::EQ_HYPERALLOC_HUGE_PAGE_SIZE,
    };
    if parts.next().is_some() {
        return Err("unexpected extra argument for VFIO DMA complete request".to_string());
    }

    let mut op_req = eqvm_defs::EqHyperAllocVfioDmaOp {
        version: eqvm_defs::EQ_HYPERALLOC_VERSION,
        op,
        status,
        instance_id: server.instance_id as u64,
        sequence,
        frame_gpa,
        frame_hpa,
        iova,
        size,
        ..eqvm_defs::EqHyperAllocVfioDmaOp::default()
    };
    ioctl::ioctl_hyperalloc_vfio_dma_complete(server.instance_fd, &mut op_req)?;
    info!(
        "microVM control HyperAlloc VFIO DMA complete request instance={} seq={} op={} status={} iova={:#x} hpa={:#x} size={:#x} errno={}",
        server.instance_id,
        op_req.sequence,
        hyperalloc_vfio_dma_op_name(op_req.op),
        hyperalloc_vfio_dma_status_name(op_req.status),
        op_req.iova,
        op_req.frame_hpa,
        op_req.size,
        op_req.result_errno
    );
    Ok(format!(
        "vfio_dma_complete seq={} op={} op_name={} status={} status_name={} frame_gpa={:#x} frame_hpa={:#x} iova={:#x} size={:#x} errno={}",
        op_req.sequence,
        op_req.op,
        hyperalloc_vfio_dma_op_name(op_req.op),
        op_req.status,
        hyperalloc_vfio_dma_status_name(op_req.status),
        op_req.frame_gpa,
        op_req.frame_hpa,
        op_req.iova,
        op_req.size,
        op_req.result_errno
    ))
}

fn parse_u64_arg(value: &str, name: &str) -> Result<u64, String> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).map_err(|err| format!("invalid {}: {}", name, err))
    } else {
        value
            .parse::<u64>()
            .map_err(|err| format!("invalid {}: {}", name, err))
    }
}

fn parse_u32_arg(value: &str, name: &str) -> Result<u32, String> {
    let parsed = parse_u64_arg(value, name)?;
    u32::try_from(parsed).map_err(|_| format!("invalid {}: {} outside u32 range", name, value))
}

fn parse_hyperalloc_vfio_dma_status(value: &str) -> Result<u32, String> {
    match value {
        "none" => Ok(eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_NONE),
        "pending" => Ok(eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING),
        "success" => Ok(eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_SUCCESS),
        "failed" => Ok(eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_FAILED),
        "unsupported" => Ok(eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED),
        _ => {
            let status = parse_u64_arg(value, "VFIO DMA status")?;
            u32::try_from(status)
                .map_err(|_| format!("invalid VFIO DMA status: {} out of range", value))
        }
    }
}

fn issue_memory_target(
    server: &ControlServer,
    target_huge_frames: u64,
    source: &str,
) -> Result<HyperAllocMemoryTargetResult, String> {
    let req = ioctl::ioctl_hyperalloc_memory_target(
        server.instance_fd,
        server.instance_id as u64,
        target_huge_frames,
        1000,
    )?;
    server
        .memory_target_huge_frames
        .store(target_huge_frames, Ordering::Release);
    info!(
        "microVM control memory-target source={} instance={} target_huge_frames={} seq={} status={} target_pages={} timeout_ms={}",
        source,
        server.instance_id,
        target_huge_frames,
        req.sequence,
        req.status,
        req.target_pages,
        req.timeout_ms
    );
    Ok(HyperAllocMemoryTargetResult {
        target_huge_frames,
        sequence: req.sequence,
        status: req.status,
        target_pages: req.target_pages,
        timeout_ms: req.timeout_ms,
    })
}

fn resize_microvm_vcpu(server: &ControlServer, count: u8) -> Result<(), String> {
    if count == 0 || count > server.max_vcpus {
        return Err(format!(
            "vCPU count {} outside allowed range 1..={}",
            count, server.max_vcpus
        ));
    }
    ioctl::ioctl_set_instance_vcpu_count(
        server.instance_fd,
        server.instance_id as u64,
        count as u32,
    )?;
    server.desired_vcpus.store(count, Ordering::Release);
    info!(
        "microVM control resize-vcpu instance={} desired={} max={}",
        server.instance_id, count, server.max_vcpus
    );
    Ok(())
}

fn format_memory_target_response(result: &HyperAllocMemoryTargetResult) -> String {
    format!(
        "memory_target_huge_frames={} seq={} status={} target_pages={} timeout_ms={}",
        result.target_huge_frames,
        result.sequence,
        result.status,
        result.target_pages,
        result.timeout_ms
    )
}

fn bool_to_u8(value: bool) -> u8 {
    if value {
        1
    } else {
        0
    }
}

fn hyperalloc_pagecache_request_outstanding(query: &eqvm_defs::EqHyperAllocQuery) -> bool {
    query.pagecache_shrink_pending_requests
        > query
            .pagecache_shrink_completed_requests
            .saturating_add(query.pagecache_shrink_failed_requests)
}

fn hyperalloc_policy_eval(
    query: &eqvm_defs::EqHyperAllocQuery,
    target_huge_frames: u64,
) -> (&'static str, &'static str) {
    if query.vfio_physical_reclaim_block_reason != 0 {
        return ("hold", "vfio_blocked");
    }
    if query.vfio_guest_ram_mmap_stale != 0 {
        return ("hold", "mmap_stale");
    }
    if hyperalloc_pagecache_request_outstanding(query) {
        return ("wait", "pending");
    }
    if query.last_pagecache_shrink_seq == 0
        || query
            .pagecache_shrink_completed_requests
            .saturating_add(query.pagecache_shrink_failed_requests)
            == 0
    {
        return ("eligible", "below_target");
    }
    if query.last_pagecache_shrink_target_huge_frames != target_huge_frames {
        return ("eligible", "target_changed");
    }
    ("hold", "stable")
}

fn hyperalloc_scheduler_eqgate_action(
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> &'static str {
    if snapshot.scheduler_enabled == 0 || snapshot.scheduler_eqgate_drain_budget == 0 {
        return "disabled";
    }
    if query.eqgate_hyperalloc_pending == 0 {
        return "hold";
    }
    "would_drain"
}

fn set_hyperalloc_policy(
    server: &ControlServer,
    target_huge_frames: u64,
    interval_ms: Option<u64>,
) -> Result<String, String> {
    let mut policy_guard = server
        .hyperalloc_policy
        .lock()
        .map_err(|err| format!("HyperAlloc host policy lock poisoned: {}", err))?;

    if target_huge_frames == 0 {
        *policy_guard = None;
        info!(
            "HyperAlloc host policy disabled by control command instance={}",
            server.instance_id
        );
        return Ok(format_hyperalloc_policy_set_response(None));
    }

    let interval_ms = clamp_hyperalloc_policy_interval_ms(
        interval_ms.unwrap_or(HYPERALLOC_POLICY_DEFAULT_INTERVAL_MS),
    );
    let interval = Duration::from_millis(interval_ms);

    match policy_guard.as_mut() {
        Some(policy) => {
            let target_changed = policy.target_huge_frames != target_huge_frames;
            let interval_changed = policy.interval != interval;
            policy.target_huge_frames = target_huge_frames;
            policy.interval = interval;
            if target_changed || interval_changed {
                policy.last_tick = Instant::now();
            }
            if target_changed {
                policy.last_issued_seq = 0;
                policy.last_completed_seq = 0;
            }
            info!(
                "HyperAlloc host policy updated by control command instance={} target_huge_frames={} interval_ms={} target_changed={} interval_changed={} last_issued_seq={} last_completed_seq={}",
                server.instance_id,
                policy.target_huge_frames,
                interval_ms,
                target_changed,
                interval_changed,
                policy.last_issued_seq,
                policy.last_completed_seq
            );
            Ok(format_hyperalloc_policy_set_response(Some(policy)))
        }
        None => {
            *policy_guard = Some(HyperAllocPolicy {
                target_huge_frames,
                interval,
                last_tick: Instant::now(),
                last_issued_seq: 0,
                last_completed_seq: 0,
            });
            let policy = policy_guard
                .as_ref()
                .ok_or_else(|| "failed to enable HyperAlloc host policy".to_string())?;
            info!(
                "HyperAlloc host policy enabled by control command instance={} target_huge_frames={} interval_ms={}",
                server.instance_id, target_huge_frames, interval_ms
            );
            Ok(format_hyperalloc_policy_set_response(Some(policy)))
        }
    }
}

fn format_hyperalloc_policy_set_response(policy: Option<&HyperAllocPolicy>) -> String {
    match policy {
        Some(policy) => format!(
            "policy_set policy_enabled=1 policy_target_huge_frames={} policy_interval_ms={} policy_last_issued_seq={} policy_last_completed_seq={}",
            policy.target_huge_frames,
            policy.interval.as_millis(),
            policy.last_issued_seq,
            policy.last_completed_seq
        ),
        None => "policy_set policy_enabled=0 policy_target_huge_frames=0 policy_interval_ms=0 policy_last_issued_seq=0 policy_last_completed_seq=0".to_string(),
    }
}

fn set_hyperalloc_metrics(server: &ControlServer, interval_ms: u64) -> Result<String, String> {
    let mut metrics_guard = server
        .hyperalloc_metrics
        .lock()
        .map_err(|err| format!("HyperAlloc metrics sampler lock poisoned: {}", err))?;

    if interval_ms == 0 {
        *metrics_guard = None;
        info!(
            "HyperAlloc metrics sampler disabled by control command instance={}",
            server.instance_id
        );
        return Ok(format_hyperalloc_metrics_set_response(None));
    }

    let interval_ms = clamp_hyperalloc_metrics_interval_ms(interval_ms);
    let interval = Duration::from_millis(interval_ms);

    match metrics_guard.as_mut() {
        Some(metrics) => {
            let interval_changed = metrics.interval != interval;
            metrics.interval = interval;
            if interval_changed {
                metrics.last_sample = Instant::now();
            }
            info!(
                "HyperAlloc metrics sampler updated by control command instance={} interval_ms={} interval_changed={}",
                server.instance_id, interval_ms, interval_changed
            );
            Ok(format_hyperalloc_metrics_set_response(Some(metrics)))
        }
        None => {
            *metrics_guard = Some(HyperAllocMetricsSampler {
                interval,
                last_sample: Instant::now(),
            });
            let metrics = metrics_guard
                .as_ref()
                .ok_or_else(|| "failed to enable HyperAlloc metrics sampler".to_string())?;
            info!(
                "HyperAlloc metrics sampler enabled by control command instance={} interval_ms={}",
                server.instance_id, interval_ms
            );
            Ok(format_hyperalloc_metrics_set_response(Some(metrics)))
        }
    }
}

fn format_hyperalloc_metrics_set_response(metrics: Option<&HyperAllocMetricsSampler>) -> String {
    match metrics {
        Some(metrics) => format!(
            "metrics_set metrics_enabled=1 metrics_interval_ms={} metrics_elapsed_ms={}",
            metrics.interval.as_millis(),
            metrics.last_sample.elapsed().as_millis()
        ),
        None => {
            "metrics_set metrics_enabled=0 metrics_interval_ms=0 metrics_elapsed_ms=0".to_string()
        }
    }
}

fn set_hyperalloc_policy_evaluator(
    server: &ControlServer,
    interval_ms: u64,
    target_huge_frames: Option<u64>,
) -> Result<String, String> {
    let mut evaluator_guard = server
        .hyperalloc_evaluator
        .lock()
        .map_err(|err| format!("HyperAlloc policy evaluator lock poisoned: {}", err))?;

    if interval_ms == 0 {
        *evaluator_guard = None;
        info!(
            "HyperAlloc policy evaluator disabled by control command instance={}",
            server.instance_id
        );
        return Ok(format_hyperalloc_eval_set_response(None));
    }

    let interval_ms = clamp_hyperalloc_eval_interval_ms(interval_ms);
    let interval = Duration::from_millis(interval_ms);
    let target_huge_frames =
        target_huge_frames.unwrap_or(HYPERALLOC_EVAL_DEFAULT_TARGET_HUGE_FRAMES);

    match evaluator_guard.as_mut() {
        Some(evaluator) => {
            let interval_changed = evaluator.interval != interval;
            let target_changed = evaluator.target_huge_frames != target_huge_frames;
            evaluator.interval = interval;
            evaluator.target_huge_frames = target_huge_frames;
            if interval_changed || target_changed {
                evaluator.last_eval = Instant::now();
            }
            info!(
                "HyperAlloc policy evaluator updated by control command instance={} interval_ms={} target_huge_frames={} interval_changed={} target_changed={}",
                server.instance_id,
                interval_ms,
                target_huge_frames,
                interval_changed,
                target_changed
            );
            Ok(format_hyperalloc_eval_set_response(Some(evaluator)))
        }
        None => {
            *evaluator_guard = Some(HyperAllocPolicyEvaluator {
                interval,
                last_eval: Instant::now(),
                target_huge_frames,
            });
            let evaluator = evaluator_guard
                .as_ref()
                .ok_or_else(|| "failed to enable HyperAlloc policy evaluator".to_string())?;
            info!(
                "HyperAlloc policy evaluator enabled by control command instance={} interval_ms={} target_huge_frames={}",
                server.instance_id, interval_ms, target_huge_frames
            );
            Ok(format_hyperalloc_eval_set_response(Some(evaluator)))
        }
    }
}

fn format_hyperalloc_eval_set_response(evaluator: Option<&HyperAllocPolicyEvaluator>) -> String {
    match evaluator {
        Some(evaluator) => format!(
            "eval_set eval_enabled=1 eval_interval_ms={} eval_elapsed_ms={} eval_target_huge_frames={}",
            evaluator.interval.as_millis(),
            evaluator.last_eval.elapsed().as_millis(),
            evaluator.target_huge_frames
        ),
        None => {
            "eval_set eval_enabled=0 eval_interval_ms=0 eval_elapsed_ms=0 eval_target_huge_frames=0"
                .to_string()
        }
    }
}

fn set_hyperalloc_scheduler(
    server: &ControlServer,
    interval_ms: u64,
    target_huge_frames: Option<u64>,
    desired_vcpus: Option<u8>,
) -> Result<String, String> {
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;

    if interval_ms == 0 {
        *scheduler_guard = None;
        info!(
            "HyperAlloc scheduler dry-run disabled by control command instance={}",
            server.instance_id
        );
        if server.has_block_backend {
            super::block::set_scheduler_block_io_weight(0);
        }
        return Ok(format_hyperalloc_scheduler_set_response(None));
    }

    let interval_ms = clamp_hyperalloc_scheduler_interval_ms(interval_ms);
    let interval = Duration::from_millis(interval_ms);
    let current_desired_vcpus = server.desired_vcpus.load(Ordering::Acquire);

    match scheduler_guard.as_mut() {
        Some(scheduler) => {
            let target_huge_frames = target_huge_frames.unwrap_or(scheduler.target_huge_frames);
            if target_huge_frames == 0 {
                return Err("scheduler target huge-frame count must be nonzero".to_string());
            }
            let desired_vcpus = desired_vcpus.unwrap_or(scheduler.desired_vcpus);
            validate_scheduler_desired_vcpus(desired_vcpus, server.max_vcpus)?;
            let interval_changed = scheduler.interval != interval;
            let target_changed = scheduler.target_huge_frames != target_huge_frames;
            let desired_changed = scheduler.desired_vcpus != desired_vcpus;
            scheduler.interval = interval;
            scheduler.target_huge_frames = target_huge_frames;
            scheduler.desired_vcpus = desired_vcpus;
            if interval_changed || target_changed || desired_changed {
                scheduler.last_tick = Instant::now();
            }
            info!(
                "HyperAlloc scheduler dry-run updated by control command instance={} interval_ms={} target_huge_frames={} desired_vcpus={} interval_changed={} target_changed={} desired_changed={}",
                server.instance_id,
                interval_ms,
                target_huge_frames,
                desired_vcpus,
                interval_changed,
                target_changed,
                desired_changed
            );
            Ok(format_hyperalloc_scheduler_set_response(Some(scheduler)))
        }
        None => {
            let target_huge_frames =
                target_huge_frames.unwrap_or(HYPERALLOC_SCHEDULER_DEFAULT_TARGET_HUGE_FRAMES);
            if target_huge_frames == 0 {
                return Err("scheduler target huge-frame count must be nonzero".to_string());
            }
            let desired_vcpus = desired_vcpus.unwrap_or(current_desired_vcpus);
            validate_scheduler_desired_vcpus(desired_vcpus, server.max_vcpus)?;
            let adaptive_vfio_queue_cap = parse_scheduler_adaptive_vfio_queue_cap_env()?;
            *scheduler_guard = Some(HyperAllocSchedulerDryRun {
                interval,
                last_tick: Instant::now(),
                target_huge_frames,
                desired_vcpus,
                vfio_queue_goal: 0,
                block_io_weight: 0,
                eqgate_drain_budget: 0,
                auto_apply: false,
                cpu_auto_apply: false,
                eqgate_auto_drain: false,
                adaptive: false,
                adaptive_vfio_queue_cap,
            });
            let scheduler = scheduler_guard
                .as_ref()
                .ok_or_else(|| "failed to enable HyperAlloc scheduler dry-run".to_string())?;
            info!(
                "HyperAlloc scheduler dry-run enabled by control command instance={} interval_ms={} target_huge_frames={} desired_vcpus={} adaptive_vfio_queue_cap={} auto_apply=0 cpu_auto_apply=0 eqgate_auto_drain=0 adaptive=0",
                server.instance_id,
                interval_ms,
                target_huge_frames,
                desired_vcpus,
                adaptive_vfio_queue_cap
            );
            Ok(format_hyperalloc_scheduler_set_response(Some(scheduler)))
        }
    }
}

fn set_hyperalloc_scheduler_io(
    server: &ControlServer,
    vfio_queue_goal: u16,
    block_io_weight: u16,
) -> Result<String, String> {
    validate_scheduler_io_goals(vfio_queue_goal, block_io_weight)?;
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let scheduler = scheduler_guard
        .as_mut()
        .ok_or_else(|| "HyperAlloc scheduler dry-run is disabled".to_string())?;
    let vfio_changed = scheduler.vfio_queue_goal != vfio_queue_goal;
    let block_changed = scheduler.block_io_weight != block_io_weight;
    scheduler.vfio_queue_goal = vfio_queue_goal;
    scheduler.block_io_weight = block_io_weight;
    if server.has_block_backend {
        super::block::set_scheduler_block_io_weight(block_io_weight);
    }
    if vfio_changed || block_changed {
        scheduler.last_tick = Instant::now();
    }
    let io_action =
        hyperalloc_scheduler_io_action(scheduler.vfio_queue_goal, scheduler.block_io_weight);
    let io_backend =
        hyperalloc_scheduler_io_backend(server.has_block_backend, server.has_vfio_backend);
    let io_effect = hyperalloc_scheduler_io_effect(
        scheduler.vfio_queue_goal,
        scheduler.block_io_weight,
        server.has_block_backend,
        server.has_vfio_backend,
    );
    info!(
        "HyperAlloc scheduler I/O policy updated by control command instance={} vfio_queue_goal={} block_io_weight={} vfio_changed={} block_changed={} io_action={} scheduler_io_backend={} io_effect={}",
        server.instance_id,
        scheduler.vfio_queue_goal,
        scheduler.block_io_weight,
        vfio_changed,
        block_changed,
        io_action,
        io_backend,
        io_effect
    );
    Ok(format!(
        "scheduler_io_set scheduler_enabled=1 scheduler_vfio_queue_goal={} scheduler_block_io_weight={} io_action={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_io_backend={} io_effect={}",
        scheduler.vfio_queue_goal,
        scheduler.block_io_weight,
        io_action,
        scheduler.interval.as_millis(),
        scheduler.last_tick.elapsed().as_millis(),
        scheduler.target_huge_frames,
        scheduler.desired_vcpus,
        io_backend,
        io_effect
    ))
}

fn set_hyperalloc_scheduler_auto_apply(
    server: &ControlServer,
    enabled: bool,
) -> Result<String, String> {
    set_hyperalloc_scheduler_memory_auto_apply_with_label(server, enabled, "scheduler_auto_set")
}

fn set_hyperalloc_scheduler_memory_auto_apply(
    server: &ControlServer,
    enabled: bool,
) -> Result<String, String> {
    set_hyperalloc_scheduler_memory_auto_apply_with_label(
        server,
        enabled,
        "scheduler_memory_auto_set",
    )
}

fn set_hyperalloc_scheduler_memory_auto_apply_with_label(
    server: &ControlServer,
    enabled: bool,
    label: &str,
) -> Result<String, String> {
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let scheduler = scheduler_guard
        .as_mut()
        .ok_or_else(|| "HyperAlloc scheduler dry-run is disabled".to_string())?;
    let changed = scheduler.auto_apply != enabled;
    scheduler.auto_apply = enabled;
    info!(
        "HyperAlloc scheduler memory auto-apply updated by control command instance={} scheduler_auto_apply={} scheduler_memory_auto_apply={} changed={}",
        server.instance_id,
        bool_to_u8(enabled),
        bool_to_u8(enabled),
        changed
    );
    Ok(format!(
        "{} scheduler_enabled=1 scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={}",
        label,
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.cpu_auto_apply),
        bool_to_u8(scheduler.eqgate_auto_drain),
        scheduler.interval.as_millis(),
        scheduler.last_tick.elapsed().as_millis(),
        scheduler.target_huge_frames,
        scheduler.desired_vcpus
    ))
}

fn set_hyperalloc_scheduler_cpu_auto_apply(
    server: &ControlServer,
    enabled: bool,
) -> Result<String, String> {
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let scheduler = scheduler_guard
        .as_mut()
        .ok_or_else(|| "HyperAlloc scheduler dry-run is disabled".to_string())?;
    let changed = scheduler.cpu_auto_apply != enabled;
    scheduler.cpu_auto_apply = enabled;
    info!(
        "HyperAlloc scheduler CPU auto-apply updated by control command instance={} scheduler_cpu_auto_apply={} changed={}",
        server.instance_id,
        bool_to_u8(enabled),
        changed
    );
    Ok(format!(
        "scheduler_cpu_auto_set scheduler_enabled=1 scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={}",
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.cpu_auto_apply),
        bool_to_u8(scheduler.eqgate_auto_drain),
        scheduler.interval.as_millis(),
        scheduler.last_tick.elapsed().as_millis(),
        scheduler.target_huge_frames,
        scheduler.desired_vcpus
    ))
}

fn set_hyperalloc_scheduler_eqgate_auto_drain(
    server: &ControlServer,
    enabled: bool,
) -> Result<String, String> {
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let scheduler = scheduler_guard
        .as_mut()
        .ok_or_else(|| "HyperAlloc scheduler dry-run is disabled".to_string())?;
    let changed = scheduler.eqgate_auto_drain != enabled;
    scheduler.eqgate_auto_drain = enabled;
    info!(
        "HyperAlloc scheduler EqGate auto-drain updated by control command instance={} scheduler_eqgate_auto_drain={} scheduler_eqgate_drain_budget={} changed={}",
        server.instance_id,
        bool_to_u8(enabled),
        scheduler.eqgate_drain_budget,
        changed
    );
    Ok(format!(
        "scheduler_eqgate_auto_set scheduler_enabled=1 scheduler_eqgate_auto_drain={} scheduler_eqgate_drain_budget={} scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={}",
        bool_to_u8(scheduler.eqgate_auto_drain),
        scheduler.eqgate_drain_budget,
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.auto_apply),
        bool_to_u8(scheduler.cpu_auto_apply),
        scheduler.interval.as_millis(),
        scheduler.last_tick.elapsed().as_millis(),
        scheduler.target_huge_frames,
        scheduler.desired_vcpus
    ))
}

fn set_hyperalloc_scheduler_adaptive(
    server: &ControlServer,
    enabled: bool,
) -> Result<String, String> {
    let mut scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let scheduler = scheduler_guard
        .as_mut()
        .ok_or_else(|| "HyperAlloc scheduler dry-run is disabled".to_string())?;
    let changed = scheduler.adaptive != enabled;
    scheduler.adaptive = enabled;
    info!(
        "HyperAlloc scheduler adaptive mode updated by control command instance={} scheduler_adaptive={} scheduler_adaptive_vfio_queue_cap={} changed={}",
        server.instance_id,
        bool_to_u8(enabled),
        scheduler.adaptive_vfio_queue_cap,
        changed
    );
    Ok(format!(
        "scheduler_adaptive_set scheduler_enabled=1 scheduler_adaptive={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_eqgate_drain_budget={} scheduler_adaptive_vfio_queue_cap={}",
        bool_to_u8(scheduler.adaptive),
        scheduler.interval.as_millis(),
        scheduler.last_tick.elapsed().as_millis(),
        scheduler.target_huge_frames,
        scheduler.desired_vcpus,
        scheduler.vfio_queue_goal,
        scheduler.block_io_weight,
        scheduler.eqgate_drain_budget,
        scheduler.adaptive_vfio_queue_cap
    ))
}

fn format_hyperalloc_scheduler_set_response(
    scheduler: Option<&HyperAllocSchedulerDryRun>,
) -> String {
    match scheduler {
        Some(scheduler) => format!(
            "scheduler_set scheduler_enabled=1 scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_eqgate_drain_budget={} scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_adaptive={} scheduler_adaptive_vfio_queue_cap={}",
            scheduler.interval.as_millis(),
            scheduler.last_tick.elapsed().as_millis(),
            scheduler.target_huge_frames,
            scheduler.desired_vcpus,
            scheduler.vfio_queue_goal,
            scheduler.block_io_weight,
            scheduler.eqgate_drain_budget,
            bool_to_u8(scheduler.auto_apply),
            bool_to_u8(scheduler.auto_apply),
            bool_to_u8(scheduler.cpu_auto_apply),
            bool_to_u8(scheduler.eqgate_auto_drain),
            bool_to_u8(scheduler.adaptive),
            scheduler.adaptive_vfio_queue_cap
        ),
        None => "scheduler_set scheduler_enabled=0 scheduler_interval_ms=0 scheduler_elapsed_ms=0 scheduler_target_huge_frames=0 scheduler_desired_vcpus=0 scheduler_vfio_queue_goal=0 scheduler_block_io_weight=0 scheduler_eqgate_drain_budget=0 scheduler_auto_apply=0 scheduler_memory_auto_apply=0 scheduler_cpu_auto_apply=0 scheduler_eqgate_auto_drain=0 scheduler_adaptive=0 scheduler_adaptive_vfio_queue_cap=0".to_string(),
    }
}

fn format_hyperalloc_scheduler_status(server: &ControlServer) -> Result<String, String> {
    let scheduler_guard = server
        .hyperalloc_scheduler
        .lock()
        .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
    let io_backend =
        hyperalloc_scheduler_io_backend(server.has_block_backend, server.has_vfio_backend);
    let response = match scheduler_guard.as_ref() {
        Some(scheduler) => {
            let io_action = hyperalloc_scheduler_io_action(
                scheduler.vfio_queue_goal,
                scheduler.block_io_weight,
            );
            format!(
                "scheduler_status scheduler_enabled=1 scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_eqgate_drain_budget={} scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_adaptive={} scheduler_adaptive_vfio_queue_cap={} io_action={} scheduler_io_backend={} io_effect={}",
                scheduler.interval.as_millis(),
                scheduler.last_tick.elapsed().as_millis(),
                scheduler.target_huge_frames,
                scheduler.desired_vcpus,
                scheduler.vfio_queue_goal,
                scheduler.block_io_weight,
                scheduler.eqgate_drain_budget,
                bool_to_u8(scheduler.auto_apply),
                bool_to_u8(scheduler.auto_apply),
                bool_to_u8(scheduler.cpu_auto_apply),
                bool_to_u8(scheduler.eqgate_auto_drain),
                bool_to_u8(scheduler.adaptive),
                scheduler.adaptive_vfio_queue_cap,
                io_action,
                io_backend,
                hyperalloc_scheduler_io_effect(
                    scheduler.vfio_queue_goal,
                    scheduler.block_io_weight,
                    server.has_block_backend,
                    server.has_vfio_backend
                )
            )
        }
        None => {
            format!(
                "scheduler_status scheduler_enabled=0 scheduler_interval_ms=0 scheduler_elapsed_ms=0 scheduler_target_huge_frames=0 scheduler_desired_vcpus=0 scheduler_vfio_queue_goal=0 scheduler_block_io_weight=0 scheduler_eqgate_drain_budget=0 scheduler_auto_apply=0 scheduler_memory_auto_apply=0 scheduler_cpu_auto_apply=0 scheduler_eqgate_auto_drain=0 scheduler_adaptive=0 scheduler_adaptive_vfio_queue_cap=0 io_action=disabled scheduler_io_backend={} io_effect=disabled",
                io_backend
            )
        }
    };
    Ok(response)
}

fn apply_hyperalloc_scheduler_once(server: &ControlServer) -> Result<String, String> {
    let snapshot = hyperalloc_runtime_snapshot(server)?;
    if snapshot.scheduler_enabled == 0 {
        return Err("HyperAlloc scheduler dry-run is disabled".to_string());
    }
    if snapshot.policy_enabled != 0 {
        return Err("disable HyperAlloc host policy before scheduler apply-once".to_string());
    }
    if snapshot.scheduler_target_huge_frames == 0 {
        return Err("scheduler target huge-frame count must be nonzero".to_string());
    }

    let query = ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64)?;
    let (decision, reason) = hyperalloc_policy_eval(&query, snapshot.scheduler_target_huge_frames);
    let (memory_action, cpu_action, io_action) = hyperalloc_scheduler_actions(&snapshot, decision);
    let io_backend =
        hyperalloc_scheduler_io_backend(snapshot.has_block_backend, snapshot.has_vfio_backend);
    let io_effect = hyperalloc_scheduler_io_effect(
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
        snapshot.has_block_backend,
        snapshot.has_vfio_backend,
    );

    if memory_action != "would_target" {
        return Ok(format!(
            "scheduler_apply memory_action={} memory_effect={} cpu_action={} io_action={} decision={} reason={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} current_memory_target_huge_frames={} seq=0 status=0 target_pages=0 timeout_ms=0 scheduler_io_backend={} io_effect={}",
            memory_action,
            hyperalloc_memory_effect(&query),
            cpu_action,
            io_action,
            decision,
            reason,
            snapshot.scheduler_target_huge_frames,
            snapshot.scheduler_desired_vcpus,
            snapshot.memory_target_huge_frames,
            io_backend,
            io_effect
        ));
    }

    let result = issue_memory_target(server, snapshot.scheduler_target_huge_frames, "scheduler")?;
    Ok(format!(
        "scheduler_apply memory_action=issued memory_effect={} cpu_action={} io_action={} decision={} reason={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} current_memory_target_huge_frames={} seq={} status={} target_pages={} timeout_ms={} scheduler_io_backend={} io_effect={}",
        hyperalloc_memory_effect(&query),
        cpu_action,
        io_action,
        decision,
        reason,
        snapshot.scheduler_target_huge_frames,
        snapshot.scheduler_desired_vcpus,
        snapshot.memory_target_huge_frames,
        result.sequence,
        result.status,
        result.target_pages,
        result.timeout_ms,
        io_backend,
        io_effect
    ))
}

fn apply_hyperalloc_scheduler_all_once(server: &ControlServer) -> Result<String, String> {
    let snapshot = hyperalloc_runtime_snapshot(server)?;
    if snapshot.scheduler_enabled == 0 {
        return Err("HyperAlloc scheduler dry-run is disabled".to_string());
    }
    if snapshot.policy_enabled != 0 {
        return Err("disable HyperAlloc host policy before scheduler apply-all-once".to_string());
    }
    if snapshot.scheduler_target_huge_frames == 0 {
        return Err("scheduler target huge-frame count must be nonzero".to_string());
    }
    if snapshot.scheduler_desired_vcpus == 0 {
        return Err("scheduler desired vCPU count must be nonzero".to_string());
    }
    validate_scheduler_desired_vcpus(snapshot.scheduler_desired_vcpus, snapshot.max_vcpus)?;

    let query = ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64)?;
    let (decision, reason) = hyperalloc_policy_eval(&query, snapshot.scheduler_target_huge_frames);
    let (memory_action, cpu_action, io_action) = hyperalloc_scheduler_actions(&snapshot, decision);
    let memory_apply =
        apply_hyperalloc_scheduler_memory_target(server, &snapshot, memory_action, reason)?;
    let cpu_apply = apply_hyperalloc_scheduler_cpu_target(server, &snapshot, cpu_action)?;
    let block_policy_result = apply_hyperalloc_scheduler_block_io_policy(&snapshot);
    let vfio_channel_result = hyperalloc_scheduler_vfio_channel_result(&snapshot);
    let eqgate_apply = apply_hyperalloc_scheduler_eqgate_drain(server, &snapshot, &query);
    let io_backend =
        hyperalloc_scheduler_io_backend(snapshot.has_block_backend, snapshot.has_vfio_backend);
    let io_effect = hyperalloc_scheduler_io_effect(
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
        snapshot.has_block_backend,
        snapshot.has_vfio_backend,
    );

    info!(
        "HyperAlloc scheduler apply-all once instance={} memory_apply_result={} cpu_apply_result={} block_policy_result={} vfio_channel_result={} eqgate_apply_result={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={}",
        server.instance_id,
        memory_apply.result,
        cpu_apply.result,
        block_policy_result,
        vfio_channel_result,
        eqgate_apply.result,
        snapshot.scheduler_target_huge_frames,
        snapshot.scheduler_desired_vcpus,
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight
    );

    Ok(format!(
        "scheduler_apply_all memory_action={} memory_apply_result={} memory_effect={} memory_seq={} memory_status={} memory_target_pages={} memory_timeout_ms={} cpu_action={} cpu_apply_result={} desired_vcpus_before={} desired_vcpus_after={} scheduler_desired_vcpus={} max_vcpus={} io_action={} scheduler_io_backend={} io_effect={} block_policy_result={} vfio_channel_result={} eqgate_action={} eqgate_apply_result={} eqgate_apply_flags={:#x} eqgate_apply_max_requests={} eqgate_apply_visited_pcpus={} eqgate_apply_pending_before={} eqgate_apply_drained={} eqgate_apply_installed={} eqgate_apply_unsupported={} eqgate_apply_failed={} eqgate_apply_pending_after={} eqgate_apply_last_seq={} eqgate_apply_skipped={} eqgate_apply_blocked_by_other_instance={} decision={} reason={} scheduler_target_huge_frames={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_eqgate_drain_budget={} current_memory_target_huge_frames={}",
        memory_action,
        memory_apply.result,
        hyperalloc_memory_effect(&query),
        memory_apply.sequence,
        memory_apply.status,
        memory_apply.target_pages,
        memory_apply.timeout_ms,
        cpu_action,
        cpu_apply.result,
        cpu_apply.desired_before,
        cpu_apply.desired_after,
        snapshot.scheduler_desired_vcpus,
        cpu_apply.max_vcpus,
        io_action,
        io_backend,
        io_effect,
        block_policy_result,
        vfio_channel_result,
        hyperalloc_scheduler_eqgate_action(&snapshot, &query),
        eqgate_apply.result,
        eqgate_apply.flags,
        eqgate_apply.max_requests,
        eqgate_apply.visited_pcpus,
        eqgate_apply.pending_before,
        eqgate_apply.drained,
        eqgate_apply.installed,
        eqgate_apply.unsupported,
        eqgate_apply.failed,
        eqgate_apply.pending_after,
        eqgate_apply.last_sequence,
        eqgate_apply.skipped,
        eqgate_apply.blocked_by_other_instance,
        decision,
        reason,
        snapshot.scheduler_target_huge_frames,
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
        snapshot.scheduler_eqgate_drain_budget,
        snapshot.memory_target_huge_frames
    ))
}

fn apply_hyperalloc_scheduler_memory_target(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    memory_action: &str,
    reason: &str,
) -> Result<HyperAllocSchedulerMemoryApplyResult, String> {
    if reason == "vfio_blocked" || reason == "mmap_stale" {
        return Ok(HyperAllocSchedulerMemoryApplyResult::zero("blocked"));
    }
    if memory_action == "wait" {
        return Ok(HyperAllocSchedulerMemoryApplyResult::zero("wait"));
    }
    if memory_action != "would_target" {
        return Ok(HyperAllocSchedulerMemoryApplyResult::zero("hold"));
    }

    let result = issue_memory_target(
        server,
        snapshot.scheduler_target_huge_frames,
        "scheduler-apply-all",
    )?;
    Ok(HyperAllocSchedulerMemoryApplyResult {
        result: "issued",
        sequence: result.sequence,
        status: result.status,
        target_pages: result.target_pages,
        timeout_ms: result.timeout_ms,
    })
}

fn apply_hyperalloc_scheduler_cpu_target(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    cpu_action: &str,
) -> Result<HyperAllocSchedulerCpuApplyResult, String> {
    if cpu_action != "would_resize" {
        return Ok(HyperAllocSchedulerCpuApplyResult::new(
            "hold",
            snapshot.desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.max_vcpus,
        ));
    }

    resize_microvm_vcpu(server, snapshot.scheduler_desired_vcpus)?;
    let desired_after = server.desired_vcpus.load(Ordering::Acquire);
    Ok(HyperAllocSchedulerCpuApplyResult::new(
        "issued",
        snapshot.desired_vcpus,
        desired_after,
        snapshot.max_vcpus,
    ))
}

fn apply_hyperalloc_scheduler_block_io_policy(
    snapshot: &HyperAllocRuntimeSnapshot,
) -> &'static str {
    if !snapshot.has_block_backend {
        if snapshot.scheduler_block_io_weight == 0 {
            return "disabled";
        }
        return "no_backend";
    }
    super::block::set_scheduler_block_io_weight(snapshot.scheduler_block_io_weight);
    if snapshot.scheduler_block_io_weight == 0 {
        "disabled"
    } else {
        "applied"
    }
}

fn hyperalloc_scheduler_vfio_channel_result(snapshot: &HyperAllocRuntimeSnapshot) -> &'static str {
    if snapshot.scheduler_vfio_queue_goal == 0 {
        "disabled"
    } else if snapshot.has_vfio_backend {
        "guest_mediated_required"
    } else {
        "no_backend"
    }
}

fn apply_hyperalloc_scheduler_eqgate_drain(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> HyperAllocSchedulerEqGateAutoDrainResult {
    if snapshot.scheduler_eqgate_drain_budget == 0 {
        return HyperAllocSchedulerEqGateAutoDrainResult::zero("disabled");
    }
    if query.eqgate_hyperalloc_pending == 0 {
        return HyperAllocSchedulerEqGateAutoDrainResult::zero("hold");
    }

    let flags = eqvm_defs::EQ_HYPERALLOC_EQGATE_DRAIN_FLAG_EXECUTE;
    match ioctl::ioctl_hyperalloc_eqgate_drain(
        server.instance_fd,
        server.instance_id as u64,
        snapshot.scheduler_eqgate_drain_budget,
        flags,
    ) {
        Ok(req) => HyperAllocSchedulerEqGateAutoDrainResult {
            result: "issued",
            flags: req.flags,
            max_requests: req.max_requests,
            visited_pcpus: req.visited_pcpus,
            pending_before: req.pending_before,
            drained: req.drained,
            installed: req.installed,
            unsupported: req.unsupported,
            failed: req.failed,
            pending_after: req.pending_after,
            last_sequence: req.last_sequence,
            skipped: req.skipped,
            blocked_by_other_instance: req.blocked_by_other_instance,
        },
        Err(err) => {
            warn!(
                "HyperAlloc scheduler apply-all EqGate drain failed instance={} budget={} pending={} err={}",
                server.instance_id,
                snapshot.scheduler_eqgate_drain_budget,
                query.eqgate_hyperalloc_pending,
                err
            );
            HyperAllocSchedulerEqGateAutoDrainResult::zero("failed")
        }
    }
}

fn apply_hyperalloc_scheduler_cpu_once(server: &ControlServer) -> Result<String, String> {
    let snapshot = hyperalloc_runtime_snapshot(server)?;
    if snapshot.scheduler_enabled == 0 {
        return Err("HyperAlloc scheduler dry-run is disabled".to_string());
    }
    if snapshot.scheduler_desired_vcpus == 0 {
        return Err("scheduler desired vCPU count must be nonzero".to_string());
    }
    validate_scheduler_desired_vcpus(snapshot.scheduler_desired_vcpus, snapshot.max_vcpus)?;

    if snapshot.desired_vcpus == snapshot.scheduler_desired_vcpus {
        return Ok(format!(
            "scheduler_cpu_apply cpu_action=hold scheduler_desired_vcpus={} desired_vcpus_before={} desired_vcpus_after={} max_vcpus={}",
            snapshot.scheduler_desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.max_vcpus
        ));
    }

    resize_microvm_vcpu(server, snapshot.scheduler_desired_vcpus)?;
    let desired_vcpus_after = server.desired_vcpus.load(Ordering::Acquire);
    Ok(format!(
        "scheduler_cpu_apply cpu_action=issued scheduler_desired_vcpus={} desired_vcpus_before={} desired_vcpus_after={} max_vcpus={}",
        snapshot.scheduler_desired_vcpus,
        snapshot.desired_vcpus,
        desired_vcpus_after,
        snapshot.max_vcpus
    ))
}

fn maybe_adapt_hyperalloc_scheduler_goals(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> HyperAllocSchedulerAdaptResult {
    let base = HyperAllocSchedulerAdaptResult::from_snapshot("disabled", snapshot);
    if snapshot.scheduler_enabled == 0 || snapshot.scheduler_adaptive == 0 {
        return base;
    }

    let target_after = adaptive_memory_target_huge_frames(snapshot, query);
    let desired_after = adaptive_desired_vcpus(snapshot, query);
    let block_weight_after = adaptive_block_io_weight(snapshot);
    let eqgate_budget_after = adaptive_eqgate_drain_budget(snapshot, query);

    let mut scheduler_guard = match server.hyperalloc_scheduler.lock() {
        Ok(guard) => guard,
        Err(err) => {
            warn!(
                "HyperAlloc scheduler lock poisoned during adaptive update: {}",
                err
            );
            return HyperAllocSchedulerAdaptResult::from_snapshot("failed", snapshot);
        }
    };
    let Some(scheduler) = scheduler_guard.as_mut() else {
        return HyperAllocSchedulerAdaptResult::from_snapshot("disabled", snapshot);
    };
    if !scheduler.adaptive {
        return HyperAllocSchedulerAdaptResult::from_snapshot("disabled", snapshot);
    }

    let target_before = scheduler.target_huge_frames;
    let desired_before = scheduler.desired_vcpus;
    let vfio_queue_before = scheduler.vfio_queue_goal;
    let vfio_queue_after = adaptive_vfio_queue_goal(
        snapshot,
        desired_after,
        scheduler.vfio_queue_goal,
        scheduler.adaptive_vfio_queue_cap,
    );
    let block_weight_before = scheduler.block_io_weight;
    let eqgate_budget_before = scheduler.eqgate_drain_budget;
    let changed = target_before != target_after
        || desired_before != desired_after
        || vfio_queue_before != vfio_queue_after
        || block_weight_before != block_weight_after
        || eqgate_budget_before != eqgate_budget_after;

    if changed {
        scheduler.target_huge_frames = target_after;
        scheduler.desired_vcpus = desired_after;
        scheduler.vfio_queue_goal = vfio_queue_after;
        scheduler.block_io_weight = block_weight_after;
        scheduler.eqgate_drain_budget = eqgate_budget_after;
        info!(
            "HyperAlloc scheduler adaptive goals updated instance={} target_huge_frames {}->{} desired_vcpus {}->{} vfio_queue_goal {}->{} adaptive_vfio_queue_cap={} block_io_weight {}->{} eqgate_drain_budget {}->{}",
            server.instance_id,
            target_before,
            target_after,
            desired_before,
            desired_after,
            vfio_queue_before,
            vfio_queue_after,
            scheduler.adaptive_vfio_queue_cap,
            block_weight_before,
            block_weight_after,
            eqgate_budget_before,
            eqgate_budget_after
        );
    }

    HyperAllocSchedulerAdaptResult {
        result: if changed { "updated" } else { "hold" },
        target_before,
        target_after,
        desired_before,
        desired_after,
        vfio_queue_before,
        vfio_queue_after,
        block_weight_before,
        block_weight_after,
        eqgate_budget_before,
        eqgate_budget_after,
    }
}

fn adaptive_memory_target_huge_frames(
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> u64 {
    if query.registered_frames == 0 {
        return snapshot.scheduler_target_huge_frames;
    }
    snapshot
        .scheduler_target_huge_frames
        .clamp(1, query.registered_frames)
}

fn adaptive_desired_vcpus(
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> u8 {
    let max_vcpus = snapshot.max_vcpus.max(1);
    let desired = snapshot.scheduler_desired_vcpus.clamp(1, max_vcpus);
    if hyperalloc_scheduler_resource_backlog(query) {
        max_vcpus
    } else {
        desired
    }
}

fn hyperalloc_scheduler_resource_backlog(query: &eqvm_defs::EqHyperAllocQuery) -> bool {
    hyperalloc_pagecache_request_outstanding(query)
        || query.reclaiming_frames != 0
        || query.vfio_dma_outstanding != 0
        || query.guest_ram_mmap_zap_outstanding != 0
        || query.eqgate_hyperalloc_pending != 0
}

fn adaptive_vfio_queue_goal(
    snapshot: &HyperAllocRuntimeSnapshot,
    desired_vcpus: u8,
    current_goal: u16,
    adaptive_cap: u16,
) -> u16 {
    if !snapshot.has_vfio_backend {
        0
    } else if current_goal != 0 {
        current_goal
    } else {
        u16::from(desired_vcpus).min(adaptive_cap)
    }
}

fn adaptive_block_io_weight(snapshot: &HyperAllocRuntimeSnapshot) -> u16 {
    if !snapshot.has_block_backend {
        0
    } else if snapshot.scheduler_block_io_weight != 0 {
        snapshot.scheduler_block_io_weight
    } else {
        HYPERALLOC_SCHEDULER_ADAPTIVE_BLOCK_IO_WEIGHT
    }
}

fn adaptive_eqgate_drain_budget(
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> u32 {
    if query.eqgate_hyperalloc_pending == 0 {
        return snapshot.scheduler_eqgate_drain_budget;
    }
    let pending_budget = query
        .eqgate_hyperalloc_pending
        .min(u64::from(HYPERALLOC_SCHEDULER_ADAPTIVE_EQGATE_DRAIN_BUDGET));
    u32::try_from(pending_budget)
        .unwrap_or(HYPERALLOC_SCHEDULER_ADAPTIVE_EQGATE_DRAIN_BUDGET)
        .clamp(1, HYPERALLOC_SCHEDULER_MAX_EQGATE_DRAIN_BUDGET)
}

fn maybe_auto_apply_hyperalloc_scheduler(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    decision: &str,
    reason: &str,
    memory_action: &str,
) -> HyperAllocSchedulerAutoApplyResult {
    if snapshot.scheduler_enabled == 0 || snapshot.scheduler_auto_apply == 0 {
        return HyperAllocSchedulerAutoApplyResult::zero("disabled");
    }
    if snapshot.policy_enabled != 0 || snapshot.scheduler_target_huge_frames == 0 {
        return HyperAllocSchedulerAutoApplyResult::zero("blocked");
    }
    if memory_action == "wait" {
        return HyperAllocSchedulerAutoApplyResult::zero("wait");
    }
    if reason == "vfio_blocked" || reason == "mmap_stale" {
        return HyperAllocSchedulerAutoApplyResult::zero("blocked");
    }
    if memory_action != "would_target" {
        return HyperAllocSchedulerAutoApplyResult::zero("hold");
    }

    match issue_memory_target(
        server,
        snapshot.scheduler_target_huge_frames,
        "scheduler-auto",
    ) {
        Ok(result) => HyperAllocSchedulerAutoApplyResult {
            result: "issued",
            sequence: result.sequence,
            status: result.status,
            target_pages: result.target_pages,
            timeout_ms: result.timeout_ms,
        },
        Err(err) => {
            warn!(
                "HyperAlloc scheduler auto-apply memory-target failed target_huge_frames={} decision={} reason={}: {}",
                snapshot.scheduler_target_huge_frames, decision, reason, err
            );
            HyperAllocSchedulerAutoApplyResult::zero("failed")
        }
    }
}

fn maybe_auto_apply_hyperalloc_scheduler_cpu(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    cpu_action: &str,
) -> HyperAllocSchedulerCpuAutoApplyResult {
    if snapshot.scheduler_enabled == 0 || snapshot.scheduler_cpu_auto_apply == 0 {
        return HyperAllocSchedulerCpuAutoApplyResult::new(
            "disabled",
            snapshot.desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.max_vcpus,
        );
    }
    if snapshot.scheduler_desired_vcpus == 0
        || validate_scheduler_desired_vcpus(snapshot.scheduler_desired_vcpus, snapshot.max_vcpus)
            .is_err()
    {
        return HyperAllocSchedulerCpuAutoApplyResult::new(
            "invalid",
            snapshot.desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.max_vcpus,
        );
    }
    if cpu_action != "would_resize" {
        return HyperAllocSchedulerCpuAutoApplyResult::new(
            "hold",
            snapshot.desired_vcpus,
            snapshot.desired_vcpus,
            snapshot.max_vcpus,
        );
    }

    let desired_before = snapshot.desired_vcpus;
    match resize_microvm_vcpu(server, snapshot.scheduler_desired_vcpus) {
        Ok(()) => {
            let desired_after = server.desired_vcpus.load(Ordering::Acquire);
            HyperAllocSchedulerCpuAutoApplyResult::new(
                "issued",
                desired_before,
                desired_after,
                snapshot.max_vcpus,
            )
        }
        Err(err) => {
            warn!(
                "HyperAlloc scheduler CPU auto-apply resize failed desired_vcpus={} max_vcpus={}: {}",
                snapshot.scheduler_desired_vcpus, snapshot.max_vcpus, err
            );
            HyperAllocSchedulerCpuAutoApplyResult::new(
                "failed",
                desired_before,
                desired_before,
                snapshot.max_vcpus,
            )
        }
    }
}

fn maybe_apply_hyperalloc_scheduler_io(
    snapshot: &HyperAllocRuntimeSnapshot,
    io_action: &str,
) -> HyperAllocSchedulerIoApplyResult {
    if snapshot.scheduler_enabled == 0 || io_action == "disabled" {
        return HyperAllocSchedulerIoApplyResult {
            block_policy_result: "disabled",
            vfio_channel_result: "disabled",
        };
    }

    HyperAllocSchedulerIoApplyResult {
        block_policy_result: apply_hyperalloc_scheduler_block_io_policy(snapshot),
        vfio_channel_result: hyperalloc_scheduler_vfio_channel_result(snapshot),
    }
}

fn maybe_auto_drain_hyperalloc_scheduler_eqgate(
    server: &ControlServer,
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
) -> HyperAllocSchedulerEqGateAutoDrainResult {
    if snapshot.scheduler_enabled == 0 || snapshot.scheduler_eqgate_auto_drain == 0 {
        return HyperAllocSchedulerEqGateAutoDrainResult::zero("disabled");
    }
    if snapshot.scheduler_eqgate_drain_budget == 0 {
        return HyperAllocSchedulerEqGateAutoDrainResult::zero("blocked");
    }
    if query.eqgate_hyperalloc_pending == 0 {
        return HyperAllocSchedulerEqGateAutoDrainResult::zero("hold");
    }

    let flags = eqvm_defs::EQ_HYPERALLOC_EQGATE_DRAIN_FLAG_EXECUTE;
    match ioctl::ioctl_hyperalloc_eqgate_drain(
        server.instance_fd,
        server.instance_id as u64,
        snapshot.scheduler_eqgate_drain_budget,
        flags,
    ) {
        Ok(req) => HyperAllocSchedulerEqGateAutoDrainResult {
            result: "issued",
            flags: req.flags,
            max_requests: req.max_requests,
            visited_pcpus: req.visited_pcpus,
            pending_before: req.pending_before,
            drained: req.drained,
            installed: req.installed,
            unsupported: req.unsupported,
            failed: req.failed,
            pending_after: req.pending_after,
            last_sequence: req.last_sequence,
            skipped: req.skipped,
            blocked_by_other_instance: req.blocked_by_other_instance,
        },
        Err(err) => {
            warn!(
                "HyperAlloc scheduler EqGate auto-drain failed instance={} budget={} pending={} err={}",
                server.instance_id,
                snapshot.scheduler_eqgate_drain_budget,
                query.eqgate_hyperalloc_pending,
                err
            );
            HyperAllocSchedulerEqGateAutoDrainResult::zero("failed")
        }
    }
}

impl HyperAllocSchedulerAutoApplyResult {
    fn zero(result: &'static str) -> Self {
        Self {
            result,
            sequence: 0,
            status: 0,
            target_pages: 0,
            timeout_ms: 0,
        }
    }
}

impl HyperAllocSchedulerCpuAutoApplyResult {
    fn new(result: &'static str, desired_before: u8, desired_after: u8, max_vcpus: u8) -> Self {
        Self {
            result,
            desired_before,
            desired_after,
            max_vcpus,
        }
    }
}

impl HyperAllocSchedulerAdaptResult {
    fn from_snapshot(result: &'static str, snapshot: &HyperAllocRuntimeSnapshot) -> Self {
        Self {
            result,
            target_before: snapshot.scheduler_target_huge_frames,
            target_after: snapshot.scheduler_target_huge_frames,
            desired_before: snapshot.scheduler_desired_vcpus,
            desired_after: snapshot.scheduler_desired_vcpus,
            vfio_queue_before: snapshot.scheduler_vfio_queue_goal,
            vfio_queue_after: snapshot.scheduler_vfio_queue_goal,
            block_weight_before: snapshot.scheduler_block_io_weight,
            block_weight_after: snapshot.scheduler_block_io_weight,
            eqgate_budget_before: snapshot.scheduler_eqgate_drain_budget,
            eqgate_budget_after: snapshot.scheduler_eqgate_drain_budget,
        }
    }
}

impl HyperAllocSchedulerEqGateAutoDrainResult {
    fn zero(result: &'static str) -> Self {
        Self {
            result,
            flags: 0,
            max_requests: 0,
            visited_pcpus: 0,
            pending_before: 0,
            drained: 0,
            installed: 0,
            unsupported: 0,
            failed: 0,
            pending_after: 0,
            last_sequence: 0,
            skipped: 0,
            blocked_by_other_instance: 0,
        }
    }
}

impl HyperAllocSchedulerMemoryApplyResult {
    fn zero(result: &'static str) -> Self {
        Self {
            result,
            sequence: 0,
            status: 0,
            target_pages: 0,
            timeout_ms: 0,
        }
    }
}

impl HyperAllocSchedulerCpuApplyResult {
    fn new(result: &'static str, desired_before: u8, desired_after: u8, max_vcpus: u8) -> Self {
        Self {
            result,
            desired_before,
            desired_after,
            max_vcpus,
        }
    }
}

fn format_hyperalloc_policy_status(server: &ControlServer) -> Result<String, String> {
    let snapshot = hyperalloc_runtime_snapshot(server)?;

    Ok(format!(
        "policy_status instance={} memory_target_huge_frames={} policy_enabled={} policy_target_huge_frames={} policy_interval_ms={} policy_elapsed_ms={} policy_last_issued_seq={} policy_last_completed_seq={} metrics_enabled={} metrics_interval_ms={} metrics_elapsed_ms={} evaluator_enabled={} evaluator_interval_ms={} evaluator_elapsed_ms={} evaluator_target_huge_frames={}",
        server.instance_id,
        snapshot.memory_target_huge_frames,
        snapshot.policy_enabled,
        snapshot.policy_target_huge_frames,
        snapshot.policy_interval_ms,
        snapshot.policy_elapsed_ms,
        snapshot.policy_last_issued_seq,
        snapshot.policy_last_completed_seq,
        snapshot.metrics_enabled,
        snapshot.metrics_interval_ms,
        snapshot.metrics_elapsed_ms,
        snapshot.evaluator_enabled,
        snapshot.evaluator_interval_ms,
        snapshot.evaluator_elapsed_ms,
        snapshot.evaluator_target_huge_frames
    ))
}

fn hyperalloc_runtime_snapshot(
    server: &ControlServer,
) -> Result<HyperAllocRuntimeSnapshot, String> {
    let desired_vcpus = server.desired_vcpus.load(Ordering::Acquire);
    let max_vcpus = server.max_vcpus;
    let memory_target_huge_frames = server.memory_target_huge_frames.load(Ordering::Acquire);

    let (
        policy_enabled,
        policy_target_huge_frames,
        policy_interval_ms,
        policy_elapsed_ms,
        policy_last_issued_seq,
        policy_last_completed_seq,
    ) = {
        let policy_guard = server
            .hyperalloc_policy
            .lock()
            .map_err(|err| format!("HyperAlloc host policy lock poisoned: {}", err))?;
        match policy_guard.as_ref() {
            Some(policy) => (
                1u8,
                policy.target_huge_frames,
                policy.interval.as_millis(),
                policy.last_tick.elapsed().as_millis(),
                policy.last_issued_seq,
                policy.last_completed_seq,
            ),
            None => (0u8, 0, 0, 0, 0, 0),
        }
    };

    let (metrics_enabled, metrics_interval_ms, metrics_elapsed_ms) = {
        let metrics_guard = server
            .hyperalloc_metrics
            .lock()
            .map_err(|err| format!("HyperAlloc metrics sampler lock poisoned: {}", err))?;
        match metrics_guard.as_ref() {
            Some(metrics) => (
                1u8,
                metrics.interval.as_millis(),
                metrics.last_sample.elapsed().as_millis(),
            ),
            None => (0u8, 0, 0),
        }
    };

    let (
        evaluator_enabled,
        evaluator_interval_ms,
        evaluator_elapsed_ms,
        evaluator_target_huge_frames,
    ) = {
        let evaluator_guard = server
            .hyperalloc_evaluator
            .lock()
            .map_err(|err| format!("HyperAlloc policy evaluator lock poisoned: {}", err))?;
        match evaluator_guard.as_ref() {
            Some(evaluator) => (
                1u8,
                evaluator.interval.as_millis(),
                evaluator.last_eval.elapsed().as_millis(),
                evaluator.target_huge_frames,
            ),
            None => (0u8, 0, 0, 0),
        }
    };

    let eval_target_huge_frames = if evaluator_enabled != 0 {
        evaluator_target_huge_frames
    } else {
        HYPERALLOC_EVAL_DEFAULT_TARGET_HUGE_FRAMES
    };

    let (
        scheduler_enabled,
        scheduler_interval_ms,
        scheduler_elapsed_ms,
        scheduler_target_huge_frames,
        scheduler_desired_vcpus,
        scheduler_vfio_queue_goal,
        scheduler_block_io_weight,
        scheduler_eqgate_drain_budget,
        scheduler_auto_apply,
        scheduler_memory_auto_apply,
        scheduler_cpu_auto_apply,
        scheduler_eqgate_auto_drain,
        scheduler_adaptive,
        scheduler_adaptive_vfio_queue_cap,
    ) = {
        let scheduler_guard = server
            .hyperalloc_scheduler
            .lock()
            .map_err(|err| format!("HyperAlloc scheduler lock poisoned: {}", err))?;
        match scheduler_guard.as_ref() {
            Some(scheduler) => (
                1u8,
                scheduler.interval.as_millis(),
                scheduler.last_tick.elapsed().as_millis(),
                scheduler.target_huge_frames,
                scheduler.desired_vcpus,
                scheduler.vfio_queue_goal,
                scheduler.block_io_weight,
                scheduler.eqgate_drain_budget,
                bool_to_u8(scheduler.auto_apply),
                bool_to_u8(scheduler.auto_apply),
                bool_to_u8(scheduler.cpu_auto_apply),
                bool_to_u8(scheduler.eqgate_auto_drain),
                bool_to_u8(scheduler.adaptive),
                scheduler.adaptive_vfio_queue_cap,
            ),
            None => (0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
        }
    };

    Ok(HyperAllocRuntimeSnapshot {
        desired_vcpus,
        max_vcpus,
        memory_target_huge_frames,
        policy_enabled,
        policy_target_huge_frames,
        policy_interval_ms,
        policy_elapsed_ms,
        policy_last_issued_seq,
        policy_last_completed_seq,
        metrics_enabled,
        metrics_interval_ms,
        metrics_elapsed_ms,
        evaluator_enabled,
        evaluator_interval_ms,
        evaluator_elapsed_ms,
        evaluator_target_huge_frames,
        eval_target_huge_frames,
        scheduler_enabled,
        scheduler_interval_ms,
        scheduler_elapsed_ms,
        scheduler_target_huge_frames,
        scheduler_desired_vcpus,
        scheduler_vfio_queue_goal,
        scheduler_block_io_weight,
        scheduler_eqgate_drain_budget,
        scheduler_auto_apply,
        scheduler_memory_auto_apply,
        scheduler_cpu_auto_apply,
        scheduler_eqgate_auto_drain,
        scheduler_adaptive,
        scheduler_adaptive_vfio_queue_cap,
        has_block_backend: server.has_block_backend,
        has_vfio_backend: server.has_vfio_backend,
    })
}

fn format_hyperalloc_scheduler_snapshot(server: &ControlServer) -> Result<String, String> {
    let snapshot = hyperalloc_runtime_snapshot(server)?;
    let query = ioctl::ioctl_hyperalloc_query(server.instance_fd, server.instance_id as u64)?;
    let eval_target = if snapshot.scheduler_enabled != 0 {
        snapshot.scheduler_target_huge_frames
    } else {
        snapshot.eval_target_huge_frames
    };
    let (decision, reason) = hyperalloc_policy_eval(&query, eval_target);
    let (memory_action, cpu_action, io_action) = hyperalloc_scheduler_actions(&snapshot, decision);
    let unregistered_frames = query.frame_count.saturating_sub(query.registered_frames);
    let eqgate_action = hyperalloc_scheduler_eqgate_action(&snapshot, &query);

    Ok(format!(
        "scheduler_snapshot instance={} memory_target_huge_frames={} policy_enabled={} policy_target_huge_frames={} policy_interval_ms={} policy_elapsed_ms={} policy_last_issued_seq={} policy_last_completed_seq={} metrics_enabled={} metrics_interval_ms={} metrics_elapsed_ms={} evaluator_enabled={} evaluator_interval_ms={} evaluator_elapsed_ms={} evaluator_target_huge_frames={} eval_target_huge_frames={} scheduler_enabled={} scheduler_interval_ms={} scheduler_elapsed_ms={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_adaptive={} scheduler_adaptive_vfio_queue_cap={} memory_action={} memory_effect={} cpu_action={} io_action={} decision={} reason={} last_seq={} pcache_req={} pcache_done={} pcache_failed={} registered_frames={} unregistered_frames={} installed={} soft={} hard={} reclaiming={} physical_releases={} physical_allocations={} physically_released_frames={} last_physical_release_hpa={} last_physical_allocation_hpa={} last_target_huge={} last_target_pages={} last_reclaimed_huge={} last_remaining_file_huge={} last_pcache_status={} last_pcache_errno={} vfio_dma_pending={} vfio_dma_done={} vfio_dma_failed={} vfio_dma_outstanding={} last_vfio_dma_seq={} last_vfio_dma_op={} last_vfio_dma_op_name={} last_vfio_dma_status={} last_vfio_dma_status_name={} last_vfio_dma_iova={} last_vfio_dma_hpa={} last_vfio_dma_size={} last_vfio_dma_errno={} reclaim_dma_rollback_attempts={} reclaim_dma_rollback_successes={} reclaim_dma_rollback_failures={} install_ept_rollback_attempts={} install_ept_rollback_successes={} install_ept_rollback_failures={} mmap_gen={} mmap_active={} mmap_current={} mmap_stale={} mmap_seq={} mmap_reason={} mmap_zap_pending={} mmap_zap_done={} mmap_zap_failed={} mmap_zap_outstanding={} last_mmap_zap_seq={} last_mmap_zap_status={} last_mmap_zap_status_name={} last_mmap_zap_gpa={} last_mmap_zap_len={} last_mmap_zap_zapped_vmas={} last_mmap_zap_zapped_bytes={} last_mmap_zap_errno={} vfio_block_reason={} vfio_block_state={} eqgate_ha_available={} eqgate_ha_pcpu_count={} eqgate_ha_capacity={} eqgate_ha_pending={} eqgate_ha_submitted={} eqgate_ha_drained={} eqgate_ha_dropped={} eqgate_ha_last_seq={} eqgate_root_drain_execute={} eqgate_guest_hcall_enqueue={} eqgate_direct_ept_iommu_update={} physical_release_allowed={} persistent_host_ram_consumers={} scheduler_io_backend={} io_effect={} scheduler_eqgate_drain_budget={} eqgate_action={}",
        server.instance_id,
        snapshot.memory_target_huge_frames,
        snapshot.policy_enabled,
        snapshot.policy_target_huge_frames,
        snapshot.policy_interval_ms,
        snapshot.policy_elapsed_ms,
        snapshot.policy_last_issued_seq,
        snapshot.policy_last_completed_seq,
        snapshot.metrics_enabled,
        snapshot.metrics_interval_ms,
        snapshot.metrics_elapsed_ms,
        snapshot.evaluator_enabled,
        snapshot.evaluator_interval_ms,
        snapshot.evaluator_elapsed_ms,
        snapshot.evaluator_target_huge_frames,
        snapshot.eval_target_huge_frames,
        snapshot.scheduler_enabled,
        snapshot.scheduler_interval_ms,
        snapshot.scheduler_elapsed_ms,
        snapshot.scheduler_target_huge_frames,
        snapshot.scheduler_desired_vcpus,
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
        snapshot.scheduler_auto_apply,
        snapshot.scheduler_memory_auto_apply,
        snapshot.scheduler_cpu_auto_apply,
        snapshot.scheduler_eqgate_auto_drain,
        snapshot.scheduler_adaptive,
        snapshot.scheduler_adaptive_vfio_queue_cap,
        memory_action,
        hyperalloc_memory_effect(&query),
        cpu_action,
        io_action,
        decision,
        reason,
        query.last_pagecache_shrink_seq,
        query.pagecache_shrink_pending_requests,
        query.pagecache_shrink_completed_requests,
        query.pagecache_shrink_failed_requests,
        query.registered_frames,
        unregistered_frames,
        query.installed_frames,
        query.soft_reclaimed_frames,
        query.hard_reclaimed_frames,
        query.reclaiming_frames,
        query.physical_releases,
        query.physical_allocations,
        query.physically_released_frames,
        query.last_physical_release_hpa,
        query.last_physical_allocation_hpa,
        query.last_pagecache_shrink_target_huge_frames,
        query.last_pagecache_shrink_target_pages,
        query.last_pagecache_shrink_reclaimed_huge_frames,
        query.last_pagecache_shrink_remaining_file_huge_frames,
        query.last_pagecache_shrink_status,
        query.last_pagecache_shrink_errno,
        query.vfio_dma_pending_requests,
        query.vfio_dma_completed_requests,
        query.vfio_dma_failed_requests,
        query.vfio_dma_outstanding,
        query.last_vfio_dma_seq,
        query.last_vfio_dma_op,
        hyperalloc_vfio_dma_op_name(query.last_vfio_dma_op),
        query.last_vfio_dma_status,
        hyperalloc_vfio_dma_status_name(query.last_vfio_dma_status),
        query.last_vfio_dma_iova,
        query.last_vfio_dma_hpa,
        query.last_vfio_dma_size,
        query.last_vfio_dma_errno,
        query.reclaim_dma_rollback_attempts,
        query.reclaim_dma_rollback_successes,
        query.reclaim_dma_rollback_failures,
        query.install_ept_rollback_attempts,
        query.install_ept_rollback_successes,
        query.install_ept_rollback_failures,
        query.vfio_guest_ram_mmap_generation,
        query.vfio_guest_ram_mmap_active,
        query.vfio_guest_ram_mmap_current,
        query.vfio_guest_ram_mmap_stale,
        query.vfio_guest_ram_mmap_update_seq,
        query.vfio_guest_ram_mmap_last_reason,
        query.guest_ram_mmap_zap_pending_requests,
        query.guest_ram_mmap_zap_completed_requests,
        query.guest_ram_mmap_zap_failed_requests,
        query.guest_ram_mmap_zap_outstanding,
        query.last_guest_ram_mmap_zap_seq,
        query.last_guest_ram_mmap_zap_status,
        hyperalloc_mmap_zap_status_name(query.last_guest_ram_mmap_zap_status),
        query.last_guest_ram_mmap_zap_gpa,
        query.last_guest_ram_mmap_zap_len,
        query.last_guest_ram_mmap_zap_zapped_vmas,
        query.last_guest_ram_mmap_zap_zapped_bytes,
        query.last_guest_ram_mmap_zap_errno,
        query.vfio_physical_reclaim_block_reason,
        hyperalloc_vfio_block_state(query.vfio_physical_reclaim_block_reason),
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_BATCH_AVAILABLE
        ),
        query.eqgate_hyperalloc_pcpu_count,
        query.eqgate_hyperalloc_queue_capacity,
        query.eqgate_hyperalloc_pending,
        query.eqgate_hyperalloc_submitted,
        query.eqgate_hyperalloc_drained,
        query.eqgate_hyperalloc_dropped,
        query.eqgate_hyperalloc_last_sequence,
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_ROOT_DRAIN_EXECUTE
        ),
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_GUEST_HCALL_ENQUEUE
        ),
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_DIRECT_EPT_IOMMU_UPDATE
        ),
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PHYSICAL_RELEASE_ALLOWED
        ),
        hyperalloc_query_flag(
            &query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PERSISTENT_HOST_RAM_CONSUMERS
        ),
        hyperalloc_scheduler_io_backend(snapshot.has_block_backend, snapshot.has_vfio_backend),
        hyperalloc_scheduler_io_effect(
            snapshot.scheduler_vfio_queue_goal,
            snapshot.scheduler_block_io_weight,
            snapshot.has_block_backend,
            snapshot.has_vfio_backend
        ),
        snapshot.scheduler_eqgate_drain_budget,
        eqgate_action
    ))
}

fn format_hyperalloc_scheduler_tick(
    snapshot: &HyperAllocRuntimeSnapshot,
    query: &eqvm_defs::EqHyperAllocQuery,
    decision: &str,
    reason: &str,
    memory_action: &str,
    cpu_action: &str,
    io_action: &str,
    adapt: &HyperAllocSchedulerAdaptResult,
    auto_apply: &HyperAllocSchedulerAutoApplyResult,
    cpu_auto_apply: &HyperAllocSchedulerCpuAutoApplyResult,
    io_apply: &HyperAllocSchedulerIoApplyResult,
    eqgate_auto_drain: &HyperAllocSchedulerEqGateAutoDrainResult,
) -> String {
    let unregistered_frames = query.frame_count.saturating_sub(query.registered_frames);
    let eqgate_action = hyperalloc_scheduler_eqgate_action(snapshot, query);
    format!(
        "scheduler_tick desired_vcpus={} max_vcpus={} memory_target_huge_frames={} scheduler_target_huge_frames={} scheduler_desired_vcpus={} scheduler_vfio_queue_goal={} scheduler_block_io_weight={} scheduler_auto_apply={} scheduler_memory_auto_apply={} scheduler_cpu_auto_apply={} scheduler_eqgate_auto_drain={} scheduler_adaptive={} scheduler_adaptive_vfio_queue_cap={} adaptive_result={} adaptive_target_before={} adaptive_target_after={} adaptive_desired_before={} adaptive_desired_after={} adaptive_vfio_queue_before={} adaptive_vfio_queue_after={} adaptive_block_weight_before={} adaptive_block_weight_after={} adaptive_eqgate_budget_before={} adaptive_eqgate_budget_after={} memory_action={} memory_effect={} cpu_action={} io_action={} policy_enabled={} policy_target_huge_frames={} metrics_enabled={} evaluator_enabled={} eval_target_huge_frames={} decision={} reason={} auto_apply_result={} auto_apply_seq={} auto_apply_status={} auto_apply_target_pages={} auto_apply_timeout_ms={} cpu_auto_apply_result={} cpu_auto_apply_desired_before={} cpu_auto_apply_desired_after={} cpu_auto_apply_max_vcpus={} block_policy_result={} vfio_channel_result={} last_seq={} pcache_req={} pcache_done={} pcache_failed={} registered_frames={} unregistered_frames={} installed={} soft={} hard={} reclaiming={} physical_releases={} physical_allocations={} physically_released_frames={} last_physical_release_hpa={} last_physical_allocation_hpa={} vfio_dma_pending={} vfio_dma_done={} vfio_dma_failed={} vfio_dma_outstanding={} last_vfio_dma_seq={} last_vfio_dma_op={} last_vfio_dma_op_name={} last_vfio_dma_status={} last_vfio_dma_status_name={} last_vfio_dma_iova={} last_vfio_dma_hpa={} last_vfio_dma_size={} last_vfio_dma_errno={} reclaim_dma_rollback_attempts={} reclaim_dma_rollback_successes={} reclaim_dma_rollback_failures={} install_ept_rollback_attempts={} install_ept_rollback_successes={} install_ept_rollback_failures={} mmap_active={} mmap_current={} mmap_stale={} mmap_zap_pending={} mmap_zap_done={} mmap_zap_failed={} mmap_zap_outstanding={} last_mmap_zap_seq={} last_mmap_zap_status={} last_mmap_zap_status_name={} last_mmap_zap_gpa={} last_mmap_zap_len={} last_mmap_zap_zapped_vmas={} last_mmap_zap_zapped_bytes={} last_mmap_zap_errno={} vfio_block_reason={} vfio_block_state={} eqgate_ha_available={} eqgate_ha_pcpu_count={} eqgate_ha_capacity={} eqgate_ha_pending={} eqgate_ha_submitted={} eqgate_ha_drained={} eqgate_ha_dropped={} eqgate_ha_last_seq={} eqgate_root_drain_execute={} eqgate_guest_hcall_enqueue={} eqgate_direct_ept_iommu_update={} physical_release_allowed={} persistent_host_ram_consumers={} scheduler_io_backend={} io_effect={} scheduler_eqgate_drain_budget={} eqgate_action={} eqgate_auto_drain_result={} eqgate_auto_drain_flags={:#x} eqgate_auto_drain_max_requests={} eqgate_auto_drain_visited_pcpus={} eqgate_auto_drain_pending_before={} eqgate_auto_drain_drained={} eqgate_auto_drain_installed={} eqgate_auto_drain_unsupported={} eqgate_auto_drain_failed={} eqgate_auto_drain_pending_after={} eqgate_auto_drain_last_seq={} eqgate_auto_drain_skipped={} eqgate_auto_drain_blocked_by_other_instance={}",
        snapshot.desired_vcpus,
        snapshot.max_vcpus,
        snapshot.memory_target_huge_frames,
        snapshot.scheduler_target_huge_frames,
        snapshot.scheduler_desired_vcpus,
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
        snapshot.scheduler_auto_apply,
        snapshot.scheduler_memory_auto_apply,
        snapshot.scheduler_cpu_auto_apply,
        snapshot.scheduler_eqgate_auto_drain,
        snapshot.scheduler_adaptive,
        snapshot.scheduler_adaptive_vfio_queue_cap,
        adapt.result,
        adapt.target_before,
        adapt.target_after,
        adapt.desired_before,
        adapt.desired_after,
        adapt.vfio_queue_before,
        adapt.vfio_queue_after,
        adapt.block_weight_before,
        adapt.block_weight_after,
        adapt.eqgate_budget_before,
        adapt.eqgate_budget_after,
        memory_action,
        hyperalloc_memory_effect(query),
        cpu_action,
        io_action,
        snapshot.policy_enabled,
        snapshot.policy_target_huge_frames,
        snapshot.metrics_enabled,
        snapshot.evaluator_enabled,
        snapshot.eval_target_huge_frames,
        decision,
        reason,
        auto_apply.result,
        auto_apply.sequence,
        auto_apply.status,
        auto_apply.target_pages,
        auto_apply.timeout_ms,
        cpu_auto_apply.result,
        cpu_auto_apply.desired_before,
        cpu_auto_apply.desired_after,
        cpu_auto_apply.max_vcpus,
        io_apply.block_policy_result,
        io_apply.vfio_channel_result,
        query.last_pagecache_shrink_seq,
        query.pagecache_shrink_pending_requests,
        query.pagecache_shrink_completed_requests,
        query.pagecache_shrink_failed_requests,
        query.registered_frames,
        unregistered_frames,
        query.installed_frames,
        query.soft_reclaimed_frames,
        query.hard_reclaimed_frames,
        query.reclaiming_frames,
        query.physical_releases,
        query.physical_allocations,
        query.physically_released_frames,
        query.last_physical_release_hpa,
        query.last_physical_allocation_hpa,
        query.vfio_dma_pending_requests,
        query.vfio_dma_completed_requests,
        query.vfio_dma_failed_requests,
        query.vfio_dma_outstanding,
        query.last_vfio_dma_seq,
        query.last_vfio_dma_op,
        hyperalloc_vfio_dma_op_name(query.last_vfio_dma_op),
        query.last_vfio_dma_status,
        hyperalloc_vfio_dma_status_name(query.last_vfio_dma_status),
        query.last_vfio_dma_iova,
        query.last_vfio_dma_hpa,
        query.last_vfio_dma_size,
        query.last_vfio_dma_errno,
        query.reclaim_dma_rollback_attempts,
        query.reclaim_dma_rollback_successes,
        query.reclaim_dma_rollback_failures,
        query.install_ept_rollback_attempts,
        query.install_ept_rollback_successes,
        query.install_ept_rollback_failures,
        query.vfio_guest_ram_mmap_active,
        query.vfio_guest_ram_mmap_current,
        query.vfio_guest_ram_mmap_stale,
        query.guest_ram_mmap_zap_pending_requests,
        query.guest_ram_mmap_zap_completed_requests,
        query.guest_ram_mmap_zap_failed_requests,
        query.guest_ram_mmap_zap_outstanding,
        query.last_guest_ram_mmap_zap_seq,
        query.last_guest_ram_mmap_zap_status,
        hyperalloc_mmap_zap_status_name(query.last_guest_ram_mmap_zap_status),
        query.last_guest_ram_mmap_zap_gpa,
        query.last_guest_ram_mmap_zap_len,
        query.last_guest_ram_mmap_zap_zapped_vmas,
        query.last_guest_ram_mmap_zap_zapped_bytes,
        query.last_guest_ram_mmap_zap_errno,
        query.vfio_physical_reclaim_block_reason,
        hyperalloc_vfio_block_state(query.vfio_physical_reclaim_block_reason),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_BATCH_AVAILABLE
        ),
        query.eqgate_hyperalloc_pcpu_count,
        query.eqgate_hyperalloc_queue_capacity,
        query.eqgate_hyperalloc_pending,
        query.eqgate_hyperalloc_submitted,
        query.eqgate_hyperalloc_drained,
        query.eqgate_hyperalloc_dropped,
        query.eqgate_hyperalloc_last_sequence,
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_ROOT_DRAIN_EXECUTE
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_GUEST_HCALL_ENQUEUE
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_DIRECT_EPT_IOMMU_UPDATE
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PHYSICAL_RELEASE_ALLOWED
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PERSISTENT_HOST_RAM_CONSUMERS
        ),
        hyperalloc_scheduler_io_backend(snapshot.has_block_backend, snapshot.has_vfio_backend),
        hyperalloc_scheduler_io_effect(
            snapshot.scheduler_vfio_queue_goal,
            snapshot.scheduler_block_io_weight,
            snapshot.has_block_backend,
            snapshot.has_vfio_backend
        ),
        snapshot.scheduler_eqgate_drain_budget,
        eqgate_action,
        eqgate_auto_drain.result,
        eqgate_auto_drain.flags,
        eqgate_auto_drain.max_requests,
        eqgate_auto_drain.visited_pcpus,
        eqgate_auto_drain.pending_before,
        eqgate_auto_drain.drained,
        eqgate_auto_drain.installed,
        eqgate_auto_drain.unsupported,
        eqgate_auto_drain.failed,
        eqgate_auto_drain.pending_after,
        eqgate_auto_drain.last_sequence,
        eqgate_auto_drain.skipped,
        eqgate_auto_drain.blocked_by_other_instance
    )
}

fn hyperalloc_scheduler_actions(
    snapshot: &HyperAllocRuntimeSnapshot,
    decision: &str,
) -> (&'static str, &'static str, &'static str) {
    if snapshot.scheduler_enabled == 0 {
        return ("disabled", "disabled", "disabled");
    }

    let memory_action = if decision == "wait" {
        "wait"
    } else if decision == "eligible"
        && snapshot.memory_target_huge_frames != snapshot.scheduler_target_huge_frames
    {
        "would_target"
    } else {
        "hold"
    };
    let cpu_action = if snapshot.desired_vcpus == snapshot.scheduler_desired_vcpus {
        "hold"
    } else {
        "would_resize"
    };
    let io_action = hyperalloc_scheduler_io_action(
        snapshot.scheduler_vfio_queue_goal,
        snapshot.scheduler_block_io_weight,
    );
    (memory_action, cpu_action, io_action)
}

fn hyperalloc_scheduler_io_action(vfio_queue_goal: u16, block_io_weight: u16) -> &'static str {
    if vfio_queue_goal == 0 && block_io_weight == 0 {
        "disabled"
    } else {
        "would_tune"
    }
}

fn hyperalloc_scheduler_io_backend(
    has_block_backend: bool,
    has_vfio_backend: bool,
) -> &'static str {
    match (has_block_backend, has_vfio_backend) {
        (false, false) => "none",
        (true, false) => "block",
        (false, true) => "vfio",
        (true, true) => "block+vfio",
    }
}

fn hyperalloc_scheduler_io_effect(
    vfio_queue_goal: u16,
    block_io_weight: u16,
    has_block_backend: bool,
    has_vfio_backend: bool,
) -> &'static str {
    let block_requested = block_io_weight != 0;
    let vfio_requested = vfio_queue_goal != 0;
    if !block_requested && !vfio_requested {
        "disabled"
    } else if block_requested && has_block_backend && vfio_requested && has_vfio_backend {
        "block_policy_applied_vfio_channel_pending"
    } else if block_requested && has_block_backend && vfio_requested {
        "block_policy_applied_vfio_no_backend"
    } else if block_requested && has_block_backend {
        "block_policy_applied"
    } else if block_requested && vfio_requested && has_vfio_backend {
        "vfio_channel_pending_block_no_backend"
    } else if vfio_requested && has_vfio_backend {
        "vfio_channel_pending"
    } else {
        "no_backend"
    }
}

fn hyperalloc_query_flag(query: &eqvm_defs::EqHyperAllocQuery, flag: u32) -> u8 {
    if query.flags & flag != 0 {
        1
    } else {
        0
    }
}

fn hyperalloc_memory_effect(query: &eqvm_defs::EqHyperAllocQuery) -> &'static str {
    let physical_release =
        query.flags & eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PHYSICAL_RELEASE_ALLOWED != 0;
    let logical_only = query.flags
        & (eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PERSISTENT_HOST_RAM_CONSUMERS
            | eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_RETAIN_HPA)
        != 0;
    match (physical_release, logical_only) {
        (true, false) => "physical_release",
        (false, true) => "logical_only",
        _ => "unknown",
    }
}

fn hyperalloc_vfio_block_state(reason: u32) -> &'static str {
    match reason {
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_NONE => "none",
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_STALE_VMA_POLICY => "stale_vma_policy",
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_GUEST_RAM_MMAP_ACTIVE => {
            "guest_ram_mmap_active"
        }
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_GUEST_RAM_MMAP_STALE => "guest_ram_mmap_stale",
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_DYNAMIC_DMA_UNSUPPORTED => {
            "dynamic_dma_unsupported"
        }
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_DYNAMIC_DMA_DISABLED => "dynamic_dma_disabled",
        eqvm_defs::EQ_HYPERALLOC_VFIO_RECLAIM_BLOCK_DMA_ROLLBACK_MISSING => "dma_rollback_missing",
        _ => "unknown",
    }
}

fn hyperalloc_vfio_dma_op_name(op: u32) -> &'static str {
    match op {
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_NONE => "none",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_MAP => "map",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_OP_UNMAP => "unmap",
        _ => "unknown",
    }
}

fn hyperalloc_vfio_dma_status_name(status: u32) -> &'static str {
    match status {
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_NONE => "none",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING => "pending",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_SUCCESS => "success",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_FAILED => "failed",
        eqvm_defs::EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED => "unsupported",
        _ => "unknown",
    }
}

fn hyperalloc_mmap_zap_status_name(status: u32) -> &'static str {
    match status {
        eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_NONE => "none",
        eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_PENDING => "pending",
        eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_SUCCESS => "success",
        eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_FAILED => "failed",
        eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_UNSUPPORTED => "unsupported",
        _ => "unknown",
    }
}

fn format_hyperalloc_policy_eval(
    query: &eqvm_defs::EqHyperAllocQuery,
    target_huge_frames: u64,
) -> String {
    let (decision, reason) = hyperalloc_policy_eval(query, target_huge_frames);
    let unregistered_frames = query.frame_count.saturating_sub(query.registered_frames);
    format!(
        "policy_eval decision={} reason={} target_huge_frames={} memory_effect={} last_seq={} pcache_req={} pcache_done={} pcache_failed={} registered_frames={} unregistered_frames={} mmap_active={} mmap_current={} mmap_stale={} vfio_block_reason={}",
        decision,
        reason,
        target_huge_frames,
        hyperalloc_memory_effect(query),
        query.last_pagecache_shrink_seq,
        query.pagecache_shrink_pending_requests,
        query.pagecache_shrink_completed_requests,
        query.pagecache_shrink_failed_requests,
        query.registered_frames,
        unregistered_frames,
        query.vfio_guest_ram_mmap_active,
        query.vfio_guest_ram_mmap_current,
        query.vfio_guest_ram_mmap_stale,
        query.vfio_physical_reclaim_block_reason
    )
}

fn format_hyperalloc_status(query: &eqvm_defs::EqHyperAllocQuery) -> String {
    let unregistered_frames = query.frame_count.saturating_sub(query.registered_frames);
    format!(
        "hyperalloc version={} flags={:#x} frames={} registered_frames={} unregistered_frames={} installed={} soft={} hard={} installing={} reclaiming={} logical_reclaims={} logical_returns={} logical_installs={} logical_reclaim_attempts={} logical_reclaim_failures={} logical_install_attempts={} logical_install_failures={} last_reclaim_us={} max_reclaim_us={} last_install_us={} max_install_us={} physical_releases={} physical_allocations={} physically_released_frames={} last_physical_release_hpa={} last_physical_allocation_hpa={} physical_release_allowed={} persistent_host_ram_consumers={} retain_hpa={} pcache_req={} pcache_done={} pcache_failed={} last_seq={} last_target_huge={} last_target_pages={} last_reclaimed_huge={} last_remaining_file_huge={} last_pcache_status={} last_pcache_errno={} vfio_dma_pending={} vfio_dma_done={} vfio_dma_failed={} vfio_dma_outstanding={} last_vfio_dma_seq={} last_vfio_dma_op={} last_vfio_dma_op_name={} last_vfio_dma_status={} last_vfio_dma_status_name={} last_vfio_dma_iova={} last_vfio_dma_hpa={} last_vfio_dma_size={} last_vfio_dma_errno={} reclaim_dma_rollback_attempts={} reclaim_dma_rollback_successes={} reclaim_dma_rollback_failures={} install_ept_rollback_attempts={} install_ept_rollback_successes={} install_ept_rollback_failures={} mmap_gen={} mmap_active={} mmap_current={} mmap_stale={} mmap_seq={} mmap_reason={} mmap_zap_pending={} mmap_zap_done={} mmap_zap_failed={} mmap_zap_outstanding={} last_mmap_zap_seq={} last_mmap_zap_status={} last_mmap_zap_status_name={} last_mmap_zap_gpa={} last_mmap_zap_len={} last_mmap_zap_zapped_vmas={} last_mmap_zap_zapped_bytes={} last_mmap_zap_errno={} vfio_block_reason={} vfio_block_state={} eqgate_ha_available={} eqgate_ha_pcpu_count={} eqgate_ha_capacity={} eqgate_ha_pending={} eqgate_ha_submitted={} eqgate_ha_drained={} eqgate_ha_dropped={} eqgate_ha_last_seq={} eqgate_root_drain_execute={} eqgate_guest_hcall_enqueue={} eqgate_direct_ept_iommu_update={} vfio_present={} vfio_dynamic_supported={} vfio_dynamic_enabled={} vfio_dma_blocked_stale_vma={} vfio_physical_reclaim_blocked={}",
        query.version,
        query.flags,
        query.frame_count,
        query.registered_frames,
        unregistered_frames,
        query.installed_frames,
        query.soft_reclaimed_frames,
        query.hard_reclaimed_frames,
        query.installing_frames,
        query.reclaiming_frames,
        query.logical_hard_reclaims,
        query.logical_returns,
        query.logical_installs,
        query.logical_hard_reclaim_attempts,
        query.logical_hard_reclaim_failures,
        query.logical_install_attempts,
        query.logical_install_failures,
        query.last_logical_reclaim_us,
        query.max_logical_reclaim_us,
        query.last_logical_install_us,
        query.max_logical_install_us,
        query.physical_releases,
        query.physical_allocations,
        query.physically_released_frames,
        query.last_physical_release_hpa,
        query.last_physical_allocation_hpa,
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PHYSICAL_RELEASE_ALLOWED
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_PERSISTENT_HOST_RAM_CONSUMERS
        ),
        hyperalloc_query_flag(query, eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_RETAIN_HPA),
        query.pagecache_shrink_pending_requests,
        query.pagecache_shrink_completed_requests,
        query.pagecache_shrink_failed_requests,
        query.last_pagecache_shrink_seq,
        query.last_pagecache_shrink_target_huge_frames,
        query.last_pagecache_shrink_target_pages,
        query.last_pagecache_shrink_reclaimed_huge_frames,
        query.last_pagecache_shrink_remaining_file_huge_frames,
        query.last_pagecache_shrink_status,
        query.last_pagecache_shrink_errno,
        query.vfio_dma_pending_requests,
        query.vfio_dma_completed_requests,
        query.vfio_dma_failed_requests,
        query.vfio_dma_outstanding,
        query.last_vfio_dma_seq,
        query.last_vfio_dma_op,
        hyperalloc_vfio_dma_op_name(query.last_vfio_dma_op),
        query.last_vfio_dma_status,
        hyperalloc_vfio_dma_status_name(query.last_vfio_dma_status),
        query.last_vfio_dma_iova,
        query.last_vfio_dma_hpa,
        query.last_vfio_dma_size,
        query.last_vfio_dma_errno,
        query.reclaim_dma_rollback_attempts,
        query.reclaim_dma_rollback_successes,
        query.reclaim_dma_rollback_failures,
        query.install_ept_rollback_attempts,
        query.install_ept_rollback_successes,
        query.install_ept_rollback_failures,
        query.vfio_guest_ram_mmap_generation,
        query.vfio_guest_ram_mmap_active,
        query.vfio_guest_ram_mmap_current,
        query.vfio_guest_ram_mmap_stale,
        query.vfio_guest_ram_mmap_update_seq,
        query.vfio_guest_ram_mmap_last_reason,
        query.guest_ram_mmap_zap_pending_requests,
        query.guest_ram_mmap_zap_completed_requests,
        query.guest_ram_mmap_zap_failed_requests,
        query.guest_ram_mmap_zap_outstanding,
        query.last_guest_ram_mmap_zap_seq,
        query.last_guest_ram_mmap_zap_status,
        hyperalloc_mmap_zap_status_name(query.last_guest_ram_mmap_zap_status),
        query.last_guest_ram_mmap_zap_gpa,
        query.last_guest_ram_mmap_zap_len,
        query.last_guest_ram_mmap_zap_zapped_vmas,
        query.last_guest_ram_mmap_zap_zapped_bytes,
        query.last_guest_ram_mmap_zap_errno,
        query.vfio_physical_reclaim_block_reason,
        hyperalloc_vfio_block_state(query.vfio_physical_reclaim_block_reason),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_BATCH_AVAILABLE
        ),
        query.eqgate_hyperalloc_pcpu_count,
        query.eqgate_hyperalloc_queue_capacity,
        query.eqgate_hyperalloc_pending,
        query.eqgate_hyperalloc_submitted,
        query.eqgate_hyperalloc_drained,
        query.eqgate_hyperalloc_dropped,
        query.eqgate_hyperalloc_last_sequence,
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_ROOT_DRAIN_EXECUTE
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_GUEST_HCALL_ENQUEUE
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_EQGATE_DIRECT_EPT_IOMMU_UPDATE
        ),
        hyperalloc_query_flag(query, eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_VFIO_PRESENT),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_DYNAMIC_SUPPORTED
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_DYNAMIC_ENABLED
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_BLOCKED_STALE_VMA
        ),
        hyperalloc_query_flag(
            query,
            eqvm_defs::EQ_HYPERALLOC_QUERY_FLAG_VFIO_PHYSICAL_RECLAIM_BLOCKED
        )
    )
}
