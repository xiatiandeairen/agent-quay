//! aq hold 及其 runner。

use std::collections::HashSet;
use std::fs::File;
use std::time::{Duration, Instant};

use aq_core::admit::{unschedulable, Demand};
use aq_core::devices::{judge, real_types, Verdict};
use aq_core::devspec::{parse_device, DeviceSpec};
use aq_core::model::{DeviceType, HoldRequest, HoldStatus, ProcId, Reason};
use aq_core::ports::{RealDevices, SpecError};
use aq_store::{HoldRow, NewHold, Store};
use aq_sys::lock::inheritable;
use serde_json::Value;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, spawn, view};

/// 一组借用要的设备，按请求顺序；真机已分到具体 id，模拟器为 None。
type Picked = Vec<(DeviceType, Option<String>)>;

pub struct HoldArgs {
    pub devices: Vec<String>,
    pub accounts: Vec<String>,
    pub wait_s: Option<u64>,
}

fn parse(a: &HoldArgs) -> AppResult<HoldRequest> {
    if a.devices.is_empty() {
        return Err(AppError::Usage(
            "name at least one device: aq hold <ios-sim|android-emu|ios-device|android-device|browser|desktop>...".into(),
        ));
    }
    let mut devices = vec![];
    let mut specs = vec![];
    for d in &a.devices {
        let (t, spec) = parse_device(d).map_err(AppError::Usage)?;
        devices.push(t);
        specs.push(spec);
    }
    if specs.iter().all(DeviceSpec::is_default) {
        specs.clear(); // 全是默认时不记，记录与以前一样
    }
    let mut seen = HashSet::new();
    for acc in &a.accounts {
        if acc.is_empty() || !seen.insert(acc) {
            return Err(AppError::Usage(format!("account repeated or empty: {acc:?}")));
        }
    }
    Ok(HoldRequest { devices, accounts: a.accounts.clone(), exclude: vec![], specs })
}

/// 提交时核对每台模拟器的版本、机型能不能组合出来；同样的写法只查一次（CASE-HOLD-14）。
/// 组合不了：rejected unschedulable，不进账本；查不了（工具出错）：只给 message 的环境错误。
fn check_specs(ctx: &Ctx, req: &HoldRequest) -> AppResult<Option<String>> {
    let mut seen = HashSet::new();
    for (i, t) in req.devices.iter().enumerate() {
        let spec = req.spec(i);
        if !seen.insert((*t, spec.clone())) {
            continue;
        }
        let Some(backend) = ctx.virtual_backend(*t) else { continue };
        match backend.check(&spec) {
            Ok(()) => {}
            Err(SpecError::Unfit(m)) => return Ok(Some(m)),
            Err(SpecError::Tool(m)) => return Err(AppError::Fatal(m)),
        }
    }
    Ok(None)
}

/// 借用者：已验证 agent 的会话主进程。认不出记为未识别（进程号 0），只按闲置收回（CASE-RECLAIM-5、CASE-RECLAIM-5）；
/// 不拿直接调用方顶替：它多半是每条命令临时起的 shell，命令一结束就退出。
fn owner(ctx: &Ctx) -> ProcId {
    ctx.agent().map_or(ProcId::UNKNOWN, |(pid, _, started_at)| ProcId { pid, started_at })
}

fn touch_used(ctx: &Ctx, id: &str) {
    let _ = File::create(ctx.dirs.used_file(id)); // 闲置起点；建不了时按 acquired_at 计
}

/// 真机：提交时探测一次，按「已声明 ∩ 此刻可用」判定（CASE-DEV-1、CASE-DEV-2）。不借真机就不探测。
fn check_real(req: &HoldRequest, ctx: &Ctx) -> Verdict {
    let types = real_types(req);
    if types.is_empty() {
        return Verdict::default();
    }
    let probe = aq_sys::device::Probe::from_env();
    let scans: Vec<_> = types.into_iter().map(|t| (t, probe.scan(t))).collect();
    judge(req, &ctx.config.cap, &scans)
}

pub fn hold(ctx: &Ctx, store: &mut Store, a: &HoldArgs) -> AppResult<(Value, i32)> {
    let mut req = parse(a)?;
    if let Some(why) = check_specs(ctx, &req)? {
        let r = Reason::Unschedulable;
        return Ok((view::rejected(r.as_str(), &why), r.exit_code()));
    }
    let v = check_real(&req, ctx);
    if let Some((reason, msg)) = &v.reject {
        return Ok((view::rejected_devices(reason.as_str(), msg, &v), reason.exit_code()));
    }
    req.exclude = v.exclude;
    if let Some(why) = unschedulable(&Demand::Hold(req.clone()), &ctx.config.cap) {
        let r = Reason::Unschedulable;
        return Ok((view::rejected(r.as_str(), &why), r.exit_code()));
    }
    let who = owner(ctx);
    let id = super::run::new_id("hd");
    let lock = store.tx(|c| {
        check::check_in(ctx, c, None)?;
        aq_store::insert_hold(c, &NewHold { id: &id, request: &req, owner: who, requested_at: ctx.now() })?;
        let lock = aq_sys::lock::try_lock_path(&ctx.dirs.lock_file(&id)).map_err(AppError::Fatal)?;
        lock.ok_or_else(|| AppError::Fatal(format!("lock of the new hold is taken: {id}")))
    })?;
    ctx.log_transition(&id, "", "waiting", &format!("submitted, holder pid {}", who.pid));
    spawn::runner(ctx, &id, inheritable(&lock).map_err(AppError::Fatal)?)?;
    drop(lock);
    let deadline = Instant::now() + Duration::from_secs(a.wait_s.unwrap_or(ctx.tuning.default_wait_s));
    wait_hold(ctx, store, &id, deadline)
}

pub fn wait_hold(ctx: &Ctx, store: &Store, id: &str, deadline: Instant) -> AppResult<(Value, i32)> {
    let mut round = 0;
    loop {
        let h = aq_store::get_hold(store.conn(), id)?.ok_or_else(|| AppError::Usage(format!("no such hold: {id}")))?;
        if h.status.is_settled() || Instant::now() >= deadline {
            if h.status == HoldStatus::Held {
                touch_if_missing(ctx, id);
            }
            let devs = view::hold_devices(store.conn(), id)?;
            return Ok((view::hold_json(&h, &devs), h.status.exit_code()));
        }
        ctx.pace(round, Some(deadline));
        round += 1;
    }
}

fn touch_if_missing(ctx: &Ctx, id: &str) {
    if !ctx.dirs.used_file(id).exists() {
        touch_used(ctx, id);
    }
}

// ---- runner ----

fn cleanup_half_made(ctx: &Ctx, store: &mut Store, h: &HoldRow) -> AppResult<()> {
    let mut types: Vec<DeviceType> = h.request.devices.iter().copied().filter(|d| d.is_virtual()).collect();
    types.dedup();
    for backend in types.into_iter().filter_map(|t| ctx.virtual_backend(t)) {
        for (name, id) in backend.list_aq().unwrap_or_default() {
            if ctx.my_hold_of(&name).as_deref() == Some(h.id.as_str()) {
                let _ = backend.delete(&id); // 删失败由定期对账兜底
                ctx.log_note(&h.id, &format!("deleting half-made {name} left by the previous runner"));
            }
        }
    }
    store.tx(|c| aq_store::clear_allocation(c, &h.id))?;
    Ok(())
}

fn allocate(ctx: &Ctx, store: &mut Store, h: &HoldRow) -> AppResult<Option<Picked>> {
    let mut round = 0;
    loop {
        let r = store.tx(|c| {
            let ck = check::check_in(ctx, c, Some(&h.id))?;
            let now = aq_store::get_hold(c, &h.id)?;
            if now.map(|x| x.status) != Some(HoldStatus::Waiting) {
                return Ok::<_, AppError>(Some(None));
            }
            let Some(adm) = ck.plan.admit.admitted.iter().find(|a| a.id == h.id) else { return Ok(None) };
            aq_store::allocate(c, &h.id, &adm.devices, &h.request.accounts)?;
            Ok(Some(Some(adm.devices.clone())))
        })?;
        match r {
            Some(v) => return Ok(v),
            None => {
                ctx.pace(round, None);
                round += 1;
            }
        }
    }
}

/// 已建好的虚拟设备：(序号, 类型, 设备 id)。
type Made = Vec<(i64, DeviceType, String)>;

fn create_sims(ctx: &Ctx, h: &HoldRow, devs: &[(DeviceType, Option<String>)]) -> Result<Made, (Made, String)> {
    let mut made = vec![];
    for (i, (t, _)) in devs.iter().enumerate() {
        let seq = i as i64 + 1;
        let Some(backend) = ctx.virtual_backend(*t) else { continue };
        match backend.create(&ctx.device_name(&h.id, seq), &h.request.spec(i)) {
            Ok(dev) => made.push((seq, *t, dev)),
            Err(e) => return Err((made, e)),
        }
    }
    Ok(made)
}

/// 删掉刚建好的虚拟设备（创建失败回滚、创建期间被撤销）。删失败由定期对账兜底。
fn discard(ctx: &Ctx, made: &Made) {
    for (_, t, dev) in made {
        if let Some(b) = ctx.virtual_backend(*t) {
            let _ = b.delete(dev);
        }
    }
}

pub fn runner(ctx: &Ctx, store: &mut Store, id: &str, _lock: File) -> AppResult<()> {
    let Some(h) = store.tx(|c| Ok::<_, AppError>(aq_store::get_hold(c, id)?))? else { return Ok(()) };
    if h.status != HoldStatus::Waiting {
        return Ok(());
    }
    cleanup_half_made(ctx, store, &h)?;
    let Some(devs) = allocate(ctx, store, &h)? else { return Ok(()) };
    let made = match create_sims(ctx, &h, &devs) {
        Ok(m) => m,
        Err((made, e)) => {
            discard(ctx, &made);
            if store.tx(|c| aq_store::reject_hold(c, id, ctx.now()))? {
                ctx.log_transition(id, "waiting", "rejected", &format!("device_error: {e}"));
            }
            return Ok(());
        }
    };
    let held = store.tx(|c| {
        if aq_store::get_hold(c, id)?.map(|x| x.status) != Some(HoldStatus::Waiting) {
            return Ok::<_, AppError>(false);
        }
        for (seq, _, dev) in &made {
            aq_store::set_device(c, id, *seq, dev)?;
        }
        Ok(aq_store::mark_held(c, id, ctx.now())?)
    })?;
    if held {
        touch_used(ctx, id);
        ctx.log_transition(id, "waiting", "held", "devices ready");
    } else {
        discard(ctx, &made); // 创建期间被撤销：删掉刚建好的
    }
    let _ = std::fs::remove_file(ctx.dirs.lock_file(id)); // held 之后不再探测这把锁
    Ok(())
}
